import { reactive } from "vue";

const VOLUME_STORAGE_KEY = "hmp.player.volume";

interface PlayerAudio {
  currentTime: number;
  duration: number;
  volume: number;
  pause(): void;
  play(): Promise<void> | void;
}

interface PlayerStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface PlayerControllerOptions {
  audio?: PlayerAudio;
  storage?: PlayerStorage;
  sendVolume?: (volume: number) => Promise<void> | void;
}

export enum PlayerControlStatus {
  idle,
  mousedown,
  dragging,
  mouseup,
}

export class PlayerController {
  readonly state = reactive({
    playing: false,
    progress: 0,
    volume: 1,
    controlStatus: PlayerControlStatus.idle,
    overlayVisible: false,
  });

  private readonly audio: PlayerAudio;
  private readonly storage?: PlayerStorage;
  private readonly sendVolume?: PlayerControllerOptions["sendVolume"];
  private progressbar?: HTMLElement;
  private dragPercent = 0;
  private animationFrame?: number;

  constructor(uri: string, options: PlayerControllerOptions = {}) {
    this.audio = options.audio ?? new Audio(uri);
    this.storage =
      options.storage ??
      (typeof localStorage === "undefined" ? undefined : localStorage);
    this.sendVolume = options.sendVolume;

    const volume = this.readVolume();
    this.state.volume = volume;
    this.audio.volume = volume;
  }

  mount = () => {
    window.addEventListener("mouseup", this.handleMouseUp);
    window.addEventListener("mousemove", this.handleMouseMove);
    this.animationFrame = requestAnimationFrame(this.renderProgress);
  };

  unmount = () => {
    window.removeEventListener("mouseup", this.handleMouseUp);
    window.removeEventListener("mousemove", this.handleMouseMove);
    if (this.animationFrame !== undefined) {
      cancelAnimationFrame(this.animationFrame);
    }
  };

  captureProgressBar = (element: unknown) => {
    this.progressbar = element instanceof HTMLElement ? element : undefined;
  };

  togglePlay = () => {
    if (this.state.playing) {
      this.audio.pause();
      this.state.playing = false;
      return;
    }
    this.audio.play();
    this.state.playing = true;
  };

  // UI -> local state/storage -> backend command.
  setVolume = (volume: number) => {
    const nextVolume = this.applyVolume(volume);
    void this.sendVolume?.(nextVolume);
  };

  // Backend snapshot -> local state/storage, without echoing a command.
  syncVolume = (volume: number) => {
    this.applyVolume(volume);
  };

  startDragging = () => {
    this.state.controlStatus = PlayerControlStatus.dragging;
  };

  setProgress = () => {
    this.state.controlStatus = PlayerControlStatus.idle;
    const progress = Math.min(1, Math.max(0, this.dragPercent));
    this.audio.currentTime = progress * this.audio.duration;
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

  private applyVolume(volume: number) {
    if (!Number.isFinite(volume)) return this.state.volume;
    const nextVolume = Math.min(1, Math.max(0, volume));
    this.state.volume = nextVolume;
    this.audio.volume = nextVolume;
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
    if (this.state.controlStatus === PlayerControlStatus.dragging) {
      this.setProgress();
    }
  };

  private handleMouseMove = (event: MouseEvent) => {
    this.dragPercent =
      (event.clientX -
        (this.progressbar?.offsetLeft ? this.progressbar.offsetLeft : 0)) /
      (this.progressbar?.clientWidth ? this.progressbar.clientWidth : 0);

    this.dragPercent = Math.min(1, Math.max(0, this.dragPercent));
  };

  private renderProgress = () => {
    if (this.state.controlStatus === PlayerControlStatus.idle) {
      this.state.progress = this.audio.currentTime / this.audio.duration;
    } else {
      this.state.progress = this.dragPercent;
    }
    this.animationFrame = requestAnimationFrame(this.renderProgress);
  };
}
