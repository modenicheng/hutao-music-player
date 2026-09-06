<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";

type ScrollDirection = "all" | "vertical" | "horizontal";

const props = withDefaults(
  defineProps<{
    width?: number | string;
    height?: number | string;
    direction?: ScrollDirection;
    /** 父级定高（滚动区填满父级）时开启：视口成为 size 查询容器，槽内可用 100cqh；
     *  内容撑高的用法（父级高度不定）不能开，containment 会把视口塌成 0 */
    fill?: boolean;
    /** 完全不渲染自定义滚动条（歌词列等沉浸式滚动区），原生滚动条本就隐藏 */
    hideScrollbar?: boolean;
  }>(),
  { width: "100%", height: "100%", direction: "all", fill: false },
);

const emit = defineEmits<{ "user-scroll": [] }>();

type Axis = "x" | "y";
type Size = { width: number; height: number };
type ScrollOffset = { left: number; top: number };
type Thumbs = { x: HTMLElement; y: HTMLElement };

const clamp = (value: number, min: number, max: number) =>
  Math.min(max, Math.max(min, value));

class ScrollBar {
  private readonly content: HTMLElement;
  private readonly inner: HTMLElement;
  private readonly thumb: Thumbs;
  private thumbSize: Size = { width: 0, height: 0 };
  private viewport: Size = { width: 0, height: 0 };
  private contentSize: Size = { width: 0, height: 0 };
  private scrollOffset: ScrollOffset = { left: 0, top: 0 };
  private resizeObserver: ResizeObserver;
  private innerResizeObserver: ResizeObserver;
  private dragging: boolean;
  private draggingAxis: Axis | null;
  /** 滚动条自动隐藏计时器 */
  private hideTimer: ReturnType<typeof setTimeout> | null = null;
  private dragStart: { x: number; y: number } & ScrollOffset = {
    x: 0,
    y: 0,
    left: 0,
    top: 0,
  };

  constructor(
    content: HTMLElement,
    inner: HTMLElement,
    thumb: Thumbs,
    private readonly direction: ScrollDirection,
  ) {
    this.content = content;
    this.inner = inner;
    this.thumb = thumb;
    this.dragging = false;
    this.draggingAxis = null;
    this.viewport = {
      width: content.clientWidth,
      height: content.clientHeight,
    };
    this.syncMetrics();
    this.updateThumbSize();

    this.resizeObserver = new ResizeObserver(this.handleResize);
    this.resizeObserver.observe(this.content);
    this.innerResizeObserver = new ResizeObserver(this.handleInnerResize);
    this.innerResizeObserver.observe(this.inner);
    this.content.addEventListener("scroll", this.handleScroll);
    this.thumb.x.addEventListener("mousedown", this.handleDraggingX);
    this.thumb.y.addEventListener("mousedown", this.handleDraggingY);
    // 悬停到滑块上（即使已淡出仍可命中）即刻亮起，方便抓取
    this.thumb.x.addEventListener("mouseenter", this.showThumbs);
    this.thumb.y.addEventListener("mouseenter", this.showThumbs);
    window.addEventListener("mousemove", this.handleMouseMove);
    window.addEventListener("mouseup", this.handleMouseUp);
  }

  clean() {
    this.content.removeEventListener("scroll", this.handleScroll);
    this.thumb.x.removeEventListener("mousedown", this.handleDraggingX);
    this.thumb.y.removeEventListener("mousedown", this.handleDraggingY);
    this.thumb.x.removeEventListener("mouseenter", this.showThumbs);
    this.thumb.y.removeEventListener("mouseenter", this.showThumbs);
    window.removeEventListener("mousemove", this.handleMouseMove);
    window.removeEventListener("mouseup", this.handleMouseUp);
    if (this.hideTimer !== null) clearTimeout(this.hideTimer);
    this.hideTimer = null;
    this.resizeObserver.disconnect();
    this.innerResizeObserver.disconnect();
  }

  /** 滚动条自动隐藏：滚动/滑块悬停/拖拽时亮起，停止交互 ~900ms 后淡出 */
  private showThumbs = () => {
    this.thumb.x.classList.add("is-active");
    this.thumb.y.classList.add("is-active");
    if (this.hideTimer !== null) clearTimeout(this.hideTimer);
    this.hideTimer = setTimeout(() => {
      this.hideTimer = null;
      this.thumb.x.classList.remove("is-active");
      this.thumb.y.classList.remove("is-active");
    }, 900);
  };

  private handleScroll = () => {
    this.showThumbs();
    this.syncMetrics();
    this.updateThumbPos();
  };

  private updateThumbPos() {
    const scrollable = {
      width: Math.max(this.contentSize.width - this.viewport.width, 0),
      height: Math.max(this.contentSize.height - this.viewport.height, 0),
    };
    const track = {
      width: Math.max(this.viewport.width - this.thumbSize.width, 0),
      height: Math.max(this.viewport.height - this.thumbSize.height, 0),
    };
    const left = this.toThumbOffset(
      this.scrollOffset.left,
      scrollable.width,
      track.width,
    );
    const top = this.toThumbOffset(
      this.scrollOffset.top,
      scrollable.height,
      track.height,
    );

    this.thumb.x.style.transform = `translateX(${left}px)`;
    this.thumb.y.style.transform = `translateY(${top}px)`;
  }

  private toThumbOffset(
    scrollOffset: number,
    scrollable: number,
    track: number,
  ) {
    return scrollable > 0 ? (scrollOffset / scrollable) * track : 0;
  }

  private updateThumbSize() {
    const minSize = {
      width: this.readMinSize(this.thumb.x, "width"),
      height: this.readMinSize(this.thumb.y, "height"),
    };
    this.thumbSize = {
      width: this.calculateThumbSize(
        this.contentSize.width,
        this.viewport.width,
        minSize.width,
      ),
      height: this.calculateThumbSize(
        this.contentSize.height,
        this.viewport.height,
        minSize.height,
      ),
    };

    this.thumb.x.style.width = `${this.thumbSize.width}px`;
    this.thumb.y.style.height = `${this.thumbSize.height}px`;

    this.updateThumbVisibility();
    this.updateThumbPos();
  }

  private readMinSize(element: HTMLElement, axis: "width" | "height") {
    const property = axis === "width" ? "minWidth" : "minHeight";
    return Number.parseFloat(getComputedStyle(element)[property]) || 0;
  }

  private calculateThumbSize(
    contentSize: number,
    viewportSize: number,
    minSize: number,
  ) {
    if (contentSize <= viewportSize) return viewportSize;
    return Math.min(
      viewportSize,
      Math.max((viewportSize / contentSize) * viewportSize, minSize),
    );
  }

  private updateThumbVisibility() {
    const hasVerticalScroll =
      this.direction !== "horizontal" &&
      this.contentSize.height > this.viewport.height;
    const hasHorizontalScroll =
      this.direction !== "vertical" &&
      this.contentSize.width > this.viewport.width;

    this.setThumbVisibility(this.thumb.y, hasVerticalScroll);
    this.setThumbVisibility(this.thumb.x, hasHorizontalScroll);
  }

  private setThumbVisibility(element: HTMLElement, visible: boolean) {
    element.style.display = visible ? "" : "none";
  }

  private handleResize = (entries: ResizeObserverEntry[]) => {
    const entry = entries[0];
    if (!entry) return;

    // scrollHeight/clientHeight 是取整值，contentRect 是分数值——
    // 不取整会出现"差 0.3px 幻影可滚"（滚动条常驻、thumb 铺满全高）
    this.viewport = {
      width: Math.round(entry.contentRect.width),
      height: Math.round(entry.contentRect.height),
    };
    this.syncMetrics();
    this.updateThumbSize();
  };

  private syncMetrics() {
    this.scrollOffset = {
      left: this.content.scrollLeft,
      top: this.content.scrollTop,
    };
    this.contentSize = {
      width: this.content.scrollWidth,
      height: this.content.scrollHeight,
    };
  }

  private handleInnerResize = () => {
    this.syncMetrics();
    this.updateThumbSize();
  };

  private handleDraggingX = (ev: MouseEvent) => {
    this.startDragging("x", ev);
  };

  private handleDraggingY = (ev: MouseEvent) => {
    this.startDragging("y", ev);
  };

  private startDragging(axis: Axis, ev: MouseEvent) {
    ev.preventDefault();
    emit("user-scroll");
    this.draggingAxis = axis;
    this.dragging = true;
    this.dragStart = {
      x: ev.clientX,
      y: ev.clientY,
      left: this.content.scrollLeft,
      top: this.content.scrollTop,
    };
  }

  private handleMouseMove = (ev: MouseEvent) => {
    const axis = this.draggingAxis;
    if (!this.dragging || axis === null) return;

    const isHorizontal = axis === "x";
    const viewportSize = isHorizontal
      ? this.viewport.width
      : this.viewport.height;
    const contentSize = isHorizontal
      ? this.contentSize.width
      : this.contentSize.height;
    const thumbSize = isHorizontal
      ? this.thumbSize.width
      : this.thumbSize.height;
    const startPointer = isHorizontal ? this.dragStart.x : this.dragStart.y;
    const startScroll = isHorizontal ? this.dragStart.left : this.dragStart.top;
    const pointer = isHorizontal ? ev.clientX : ev.clientY;
    const track = viewportSize - thumbSize;
    const scrollable = contentSize - viewportSize;

    if (track <= 0 || scrollable <= 0) return;

    const nextScroll = clamp(
      startScroll + ((pointer - startPointer) / track) * scrollable,
      0,
      scrollable,
    );
    if (isHorizontal) this.content.scrollLeft = nextScroll;
    else this.content.scrollTop = nextScroll;
  };

  private handleMouseUp = () => {
    if (!this.dragging) return;

    this.dragging = false;
    this.draggingAxis = null;
  };
}

const contentRef = ref<HTMLElement>();
const scrollBar = ref<ScrollBar>();
const thumbXRef = ref<HTMLElement>();
const thumbYRef = ref<HTMLElement>();
const innerRef = ref<HTMLElement>();

onMounted(() => {
  if (
    contentRef.value &&
    thumbXRef.value &&
    thumbYRef.value &&
    innerRef.value
  ) {
    scrollBar.value = new ScrollBar(
      contentRef.value,
      innerRef.value,
      {
        x: thumbXRef.value,
        y: thumbYRef.value,
      },
      props.direction,
    );
  }
});

onUnmounted(() => {
  scrollBar.value?.clean();
});

/** 内部真正滚动的视口元素：外部做程序化滚动（如歌词跟随）时需要拿到它 */
defineExpose({ viewport: contentRef });
</script>
<template>
  <div
    class="container"
    :style="{
      width: typeof props.width === 'number' ? `${props.width}px` : props.width,
      height:
        typeof props.height === 'number' ? `${props.height}px` : props.height,
    }"
  >
    <!-- 隐藏滚动条时不渲染 thumb：ref 缺位让 ScrollBar 整体不构建，零开销 -->
    <div v-if="!props.hideScrollbar" class="thumb thumb-y" ref="thumbYRef"></div>
    <div v-if="!props.hideScrollbar" class="thumb thumb-x" ref="thumbXRef"></div>
    <div
      class="content"
      :class="{ 'is-fill': props.fill }"
      :style="{
        overflowX: props.direction === 'vertical' ? 'hidden' : undefined,
        overflowY: props.direction === 'horizontal' ? 'hidden' : undefined,
      }"
      ref="contentRef"
    >
      <div
        ref="innerRef"
        class="inner"
        :class="{ 'is-horizontal': props.direction === 'horizontal' }"
      >
        <slot />
      </div>
    </div>
  </div>
</template>

<style scoped>
.container {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
}

/* 滚动条自动隐藏：is-active 由 JS 在滚动/滑块悬停/拖拽时挂上，
   闲置 900ms 后摘除，滑块以淡出收尾 */
.thumb {
  position: absolute;
  background-color: var(--neutral-600);
  opacity: 0;
  border-radius: var(--radius-full);
  transition: opacity var(--duration-fast) 0.6s;
  z-index: 1;
}

.thumb.is-active {
  opacity: 0.7;
  transition: opacity var(--duration-fast);
}

.thumb:hover {
  opacity: 0.85;
  transition: opacity var(--duration-fast);
}

.thumb-y {
  width: 0.4rem;
  min-height: 1.5rem;
  top: 0;
  right: 0rem;
}

.thumb-x {
  height: 0.4rem;
  min-width: 1.5rem;
  bottom: 0rem;
  left: 0;
}

.content {
  width: 100%;
  height: 100%;
  overflow: auto;
  scrollbar-width: none;
}
.content.is-fill {
  /* size 查询容器：槽内内容用 100cqh 拿视口高度（如播放页占满首屏的 Hero），
     不依赖中间层高度链 */
  container-type: size;
}
.content::-webkit-scrollbar {
  display: none;
}

/* .inner 必须随内容增长（不能定高/定宽），否则内容变化不会触发
   ResizeObserver，滚动条尺寸会停留在挂载时的旧值 */
.inner {
  min-height: 100%;
}

/* 横滚时宽度跟着内容走，纵向交给 overflow hidden */
.inner.is-horizontal {
  width: max-content;
  min-width: 100%;
}
</style>
