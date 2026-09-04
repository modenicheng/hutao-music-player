import { reactive, type InjectionKey } from "vue";

const VOLUME_STORAGE_KEY = "hmp.player.volume";

/** PlayerController 的 provide/inject 键：App 提供，路由页消费 */
export const playerKey: InjectionKey<PlayerController> = Symbol("player");

export interface PlayerStateSnapshot {
  status: string;
  positionMs: number;
  durationMs: number | null;
  volume: number;
  canSeek: boolean;
  canGoNext: boolean;
  canGoPrevious: boolean;
  title: string | null;
  artists: string[];
  error: string | null;
}

/** 播放队列里的一首曲（播放页/播放列表抽屉消费的最小元数据） */
export interface QueueItem {
  mid: string;
  title: string;
  artists: string[];
  album: string | null;
  coverUrl: string | null;
  durationMs: number;
}

export interface PlayerBridge {
  getState(): Promise<PlayerStateSnapshot>;
  onStateChanged(
    listener: (state: PlayerStateSnapshot) => void,
  ): Promise<() => void>;
  onError(listener: (message: string) => void): Promise<() => void>;
  togglePlay(): Promise<void>;
  seek(positionMs: number): Promise<void>;
  setVolume(volume: number): Promise<void>;
  previous(): Promise<void>;
  next(): Promise<void>;
  stop(): Promise<void>;
  // —— 附加式扩展（DESIGN.md §4）：实现方可选择性提供 ——
  getCurrentTrack?(): Promise<QueueItem | null>;
  getQueue?(): Promise<QueueItem[]>;
  playAt?(index: number): Promise<void>;
  removeAt?(index: number): Promise<void>;
  /** 用传入曲目替换当前队列并从 startIndex 播（对应未来 daemon 的 QueueAppend+Play） */
  playTracks?(tracks: QueueItem[], startIndex: number): Promise<void>;
  onQueueChanged?(
    listener: (queue: QueueItem[]) => void,
  ): Promise<() => void>;
}

interface PlayerStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface PlayerControllerOptions {
  storage?: PlayerStorage;
}

export enum PlayerControlStatus {
  idle,
  dragging,
}

export class PlayerController {
  readonly state = reactive({
    playing: false,
    progress: 0,
    positionMs: 0,
    durationMs: null as number | null,
    volume: 1,
    status: "empty",
    canSeek: false,
    canGoNext: false,
    canGoPrevious: false,
    title: null as string | null,
    artists: [] as string[],
    error: null as string | null,
    controlStatus: PlayerControlStatus.idle,
    overlayVisible: false,
    queueVisible: false,
    // 附加式扩展的镜像状态：桥未提供对应方法时保持为空
    currentTrack: null as QueueItem | null,
    queue: [] as QueueItem[],
  });

  private readonly bridge: PlayerBridge;
  private readonly storage?: PlayerStorage;
  private progressbar?: HTMLElement;
  private dragPercent = 0;
  private unsubscribes: Array<() => void> = [];

  constructor(bridge: PlayerBridge, options: PlayerControllerOptions = {}) {
    this.bridge = bridge;
    this.storage =
      options.storage ??
      (typeof localStorage === "undefined" ? undefined : localStorage);
    this.state.volume = this.readVolume();
  }

  mount = async () => {
    if (typeof window !== "undefined") {
      window.addEventListener("mouseup", this.handleMouseUp);
      window.addEventListener("mousemove", this.handleMouseMove);
    }
    try {
      const subscriptions = [
        this.bridge.onStateChanged(this.applySnapshot),
        this.bridge.onError((message) => {
          this.state.error = message;
        }),
      ] as Array<Promise<() => void>>;
      if (this.bridge.onQueueChanged) {
        subscriptions.push(this.bridge.onQueueChanged(this.applyQueue));
      }
      this.unsubscribes = await Promise.all(subscriptions);
      this.applySnapshot(await this.bridge.getState());
      if (this.bridge.getCurrentTrack) {
        this.state.currentTrack = await this.bridge.getCurrentTrack();
      }
      if (this.bridge.getQueue) {
        this.state.queue = [...(await this.bridge.getQueue())];
      }
    } catch (error) {
      this.setError(error);
    }
  };

  unmount = () => {
    if (typeof window !== "undefined") {
      window.removeEventListener("mouseup", this.handleMouseUp);
      window.removeEventListener("mousemove", this.handleMouseMove);
    }
    this.unsubscribes.forEach((unsubscribe) => unsubscribe());
    this.unsubscribes = [];
  };

  captureProgressBar = (element: unknown) => {
    this.progressbar = element instanceof HTMLElement ? element : undefined;
  };

  togglePlay = () => this.run(() => this.bridge.togglePlay());
  previous = () => this.run(() => this.bridge.previous());
  next = () => this.run(() => this.bridge.next());
  stop = () => this.run(() => this.bridge.stop());

  playAt = (index: number) => {
    if (!this.bridge.playAt) return;
    this.run(() => this.bridge.playAt!(index));
  };

  removeAt = (index: number) => {
    if (!this.bridge.removeAt) return;
    this.run(() => this.bridge.removeAt!(index));
  };

  playTracks = (tracks: QueueItem[], startIndex: number) => {
    if (!this.bridge.playTracks) return;
    this.run(() => this.bridge.playTracks!(tracks, startIndex));
  };

  seek = (positionMs: number) => {
    this.run(() => this.bridge.seek(Math.max(0, positionMs)));
  };

  private applyQueue = async (queue: QueueItem[]) => {
    this.state.queue = [...queue];
    // 队列事件同时承担"当前曲目变更"信号：重拉以确保播放页/播放条同步
    if (this.bridge.getCurrentTrack) {
      this.state.currentTrack = await this.bridge.getCurrentTrack();
    }
  };

  setVolume = (volume: number) => {
    const nextVolume = this.applyVolume(volume);
    this.run(() => this.bridge.setVolume(nextVolume));
  };

  // Daemon snapshots are authoritative and must never echo another command.
  syncVolume = (volume: number) => {
    this.applyVolume(volume);
  };

  startDragging = () => {
    if (this.state.canSeek) {
      this.state.controlStatus = PlayerControlStatus.dragging;
    }
  };

  updateDragPercent = (percent: number) => {
    if (!Number.isFinite(percent)) return;
    this.dragPercent = Math.min(1, Math.max(0, percent));
    if (this.state.controlStatus === PlayerControlStatus.dragging) {
      this.state.progress = this.dragPercent;
    }
  };

  setProgress = () => {
    if (this.state.controlStatus !== PlayerControlStatus.dragging) return;
    this.state.controlStatus = PlayerControlStatus.idle;
    const duration = this.state.durationMs;
    if (duration === null || duration <= 0) return;
    const positionMs = Math.round(duration * this.dragPercent);
    this.state.positionMs = positionMs;
    this.state.progress = this.dragPercent;
    this.run(() => this.bridge.seek(positionMs));
  };

  showOverlay = () => {
    this.state.overlayVisible = true;
  };

  hideOverlay = () => {
    this.state.overlayVisible = false;
  };

  toggleOverlay = () => {
    this.state.overlayVisible = !this.state.overlayVisible;
  };

  /** 播放页刻度条/键盘统一入口：按 0..1 比例 seek */
  seekToPercent = (percent: number) => {
    const duration = this.state.durationMs;
    if (duration === null || duration <= 0) return;
    const clamped = Math.min(1, Math.max(0, percent));
    this.state.positionMs = Math.round(duration * clamped);
    this.state.progress = clamped;
    this.run(() => this.bridge.seek(this.state.positionMs));
  };

  showQueue = () => {
    this.state.queueVisible = true;
  };

  hideQueue = () => {
    this.state.queueVisible = false;
  };

  private applySnapshot = (snapshot: PlayerStateSnapshot) => {
    this.state.status = snapshot.status;
    this.state.playing = snapshot.status === "playing";
    this.state.positionMs = snapshot.positionMs;
    this.state.durationMs = snapshot.durationMs;
    this.state.canSeek = snapshot.canSeek;
    this.state.canGoNext = snapshot.canGoNext;
    this.state.canGoPrevious = snapshot.canGoPrevious;
    this.state.title = snapshot.title;
    this.state.artists = [...snapshot.artists];
    this.state.error = snapshot.error;
    this.syncVolume(snapshot.volume);

    if (this.state.controlStatus === PlayerControlStatus.idle) {
      const duration = snapshot.durationMs;
      this.state.progress =
        duration !== null && duration > 0
          ? Math.min(1, Math.max(0, snapshot.positionMs / duration))
          : 0;
    }
  };

  private applyVolume(volume: number) {
    if (!Number.isFinite(volume)) return this.state.volume;
    const nextVolume = Math.min(1, Math.max(0, volume));
    this.state.volume = nextVolume;
    this.storage?.setItem(VOLUME_STORAGE_KEY, String(nextVolume));
    return nextVolume;
  }

  private readVolume() {
    const storedVolume = this.storage?.getItem(VOLUME_STORAGE_KEY);
    if (storedVolume === null || storedVolume === undefined) return 1;
    const volume = Number(storedVolume);
    return Number.isFinite(volume) ? Math.min(1, Math.max(0, volume)) : 1;
  }

  private handleMouseUp = () => {
    this.setProgress();
  };

  private handleMouseMove = (event: MouseEvent) => {
    const width = this.progressbar?.clientWidth ?? 0;
    if (width <= 0) return;
    const left = this.progressbar?.getBoundingClientRect().left ?? 0;
    this.updateDragPercent((event.clientX - left) / width);
  };

  private run(command: () => Promise<void>) {
    this.state.error = null;
    void command().catch((error) => this.setError(error));
  }

  private setError(error: unknown) {
    this.state.error = error instanceof Error ? error.message : String(error);
  }
}
