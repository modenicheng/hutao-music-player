import type { PlayerBridge, PlayerStateSnapshot, QueueItem } from "./player.ts";
import { lyricsOf, songPool } from "./api/mock-data.ts";
import type { SongRef } from "./api/types.ts";

/**
 * 浏览器模拟播放桥：纯前端状态机（无 Tauri / 无音频输出），
 * 让 UI 在浏览器里完整可玩——计时推进、自动切下一首、队列操作齐全。
 * 数据复用 mock API 的曲目池，保证播放页/评论/歌词的世界一致。
 */
export function songToQueueItem(song: SongRef): QueueItem {
  return {
    mid: song.mid,
    title: song.title,
    artists: song.artists.map((artist) => artist.name),
    album: song.album.name,
    coverUrl: song.album.picUrl,
    durationMs: song.durationMs,
  };
}

/** 队列首曲选歌词最丰富的一首，保证播放页开箱有完整歌词可看 */
function buildQueue(): QueueItem[] {
  const withFullLyrics = songPool.find((song) => {
    const lyrics = lyricsOf(song.mid);
    return (lyrics?.lines.length ?? 0) > 10;
  });
  const rest = songPool.filter((song) => song.mid !== withFullLyrics?.mid);
  const picked: SongRef[] = withFullLyrics ? [withFullLyrics] : [];
  // 均匀取样 8 首：步长按池子大小折算，曲风/封面足够多样
  const step = Math.max(1, Math.floor(rest.length / 8));
  for (let i = 0; picked.length < 8 && i < rest.length; i += step) {
    picked.push(rest[i]);
  }
  return picked.map(songToQueueItem);
}

const TICK_MS = 250;

export class BrowserPlayerBridge implements PlayerBridge {
  private queue: QueueItem[] = buildQueue();
  private currentIndex = 0;
  private playing = false;
  private positionMs = 0;
  private volume = 1;
  private stateListeners = new Set<(state: PlayerStateSnapshot) => void>();
  private queueListeners = new Set<(queue: QueueItem[]) => void>();
  private ticker: ReturnType<typeof setInterval> | null = null;

  async getState(): Promise<PlayerStateSnapshot> {
    return this.snapshot();
  }

  async onStateChanged(listener: (state: PlayerStateSnapshot) => void) {
    this.stateListeners.add(listener);
    listener(this.snapshot());
    return () => this.stateListeners.delete(listener);
  }

  async onError() {
    return () => undefined;
  }

  async togglePlay() {
    this.playing = !this.playing;
    this.syncTicker();
    this.emitState();
  }

  async seek(positionMs: number) {
    this.positionMs = Math.max(0, Math.min(this.duration(), positionMs));
    this.emitState();
  }

  async setVolume(volume: number) {
    this.volume = Math.min(1, Math.max(0, volume));
    this.emitState();
  }

  async previous() {
    this.currentIndex =
      (this.currentIndex - 1 + this.queue.length) % this.queue.length;
    this.startCurrent();
  }

  async next() {
    this.advance();
  }

  async stop() {
    this.playing = false;
    this.positionMs = 0;
    this.syncTicker();
    this.emitState();
  }

  async getCurrentTrack(): Promise<QueueItem | null> {
    return this.queue[this.currentIndex] ?? null;
  }

  async getQueue(): Promise<QueueItem[]> {
    return [...this.queue];
  }

  async playAt(index: number) {
    if (index < 0 || index >= this.queue.length) return;
    this.currentIndex = index;
    this.startCurrent();
  }

  async removeAt(index: number) {
    if (index < 0 || index >= this.queue.length) return;
    this.queue.splice(index, 1);
    if (this.currentIndex >= index) {
      this.currentIndex = Math.max(0, this.currentIndex - 1);
    }
    if (this.queue.length === 0) {
      this.playing = false;
      this.positionMs = 0;
      this.syncTicker();
    }
    this.emitState();
    this.emitQueue();
  }

  /** 替换队列并开播（对应未来 daemon 的 QueueAppend+Play） */
  async playTracks(tracks: QueueItem[], startIndex: number) {
    if (tracks.length === 0) return;
    this.queue = [...tracks];
    this.currentIndex = Math.min(Math.max(0, startIndex), tracks.length - 1);
    this.startCurrent();
  }

  async clear() {
    this.queue = [];
    this.currentIndex = 0;
    this.playing = false;
    this.positionMs = 0;
    this.syncTicker();
    this.emitState();
    this.emitQueue();
  }

  async onQueueChanged(listener: (queue: QueueItem[]) => void) {
    this.queueListeners.add(listener);
    listener([...this.queue]);
    return () => this.queueListeners.delete(listener);
  }

  // —— 内部 ——

  private duration() {
    return this.queue[this.currentIndex]?.durationMs ?? 0;
  }

  private snapshot(): PlayerStateSnapshot {
    const current = this.queue[this.currentIndex];
    return {
      status: current ? (this.playing ? "playing" : "paused") : "empty",
      positionMs: this.positionMs,
      durationMs: current ? current.durationMs : null,
      volume: this.volume,
      canSeek: current !== undefined,
      canGoNext: this.queue.length > 1,
      canGoPrevious: this.queue.length > 1,
      title: current?.title ?? null,
      artists: current?.artists ?? [],
      error: null,
    };
  }

  private startCurrent() {
    this.positionMs = 0;
    this.playing = true;
    this.syncTicker();
    this.emitState();
    this.emitQueue();
  }

  private advance() {
    if (this.queue.length === 0) return;
    this.currentIndex = (this.currentIndex + 1) % this.queue.length;
    this.positionMs = 0;
    this.emitState();
    this.emitQueue();
  }

  /** 曲目播完自动接下一首（循环） */
  private syncTicker = () => {
    if (this.playing && this.ticker === null) {
      this.ticker = setInterval(() => {
        this.positionMs += TICK_MS;
        if (this.positionMs >= this.duration()) {
          this.advance();
        } else {
          this.emitState();
        }
      }, TICK_MS);
    } else if (!this.playing && this.ticker !== null) {
      clearInterval(this.ticker);
      this.ticker = null;
    }
  };

  private emitState() {
    const snapshot = this.snapshot();
    this.stateListeners.forEach((listener) => listener(snapshot));
  }

  private emitQueue() {
    this.queueListeners.forEach((listener) => listener([...this.queue]));
  }
}
