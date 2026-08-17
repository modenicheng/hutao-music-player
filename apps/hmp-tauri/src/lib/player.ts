import { reactive } from "vue";

const VOLUME_STORAGE_KEY = "hmp.player.volume";

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
      this.unsubscribes = await Promise.all([
        this.bridge.onStateChanged(this.applySnapshot),
        this.bridge.onError((message) => {
          this.state.error = message;
        }),
      ]);
      this.applySnapshot(await this.bridge.getState());
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
