<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";

const props = defineProps<{
  width?: number | string;
  height?: number | string;
}>();

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
  private dragStart: { x: number; y: number } & ScrollOffset = {
    x: 0,
    y: 0,
    left: 0,
    top: 0,
  };

  constructor(
    content: HTMLElement,
    inner: HTMLElement,
    thumb: { y: HTMLElement; x: HTMLElement },
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
    window.addEventListener("mousemove", this.handleMouseMove);
    window.addEventListener("mouseup", this.handleMouseUp);
  }

  clean() {
    this.content.removeEventListener("scroll", this.handleScroll);
    this.thumb.x.removeEventListener("mousedown", this.handleDraggingX);
    this.thumb.y.removeEventListener("mousedown", this.handleDraggingY);
    window.removeEventListener("mousemove", this.handleMouseMove);
    window.removeEventListener("mouseup", this.handleMouseUp);
    this.resizeObserver.disconnect();
    this.innerResizeObserver.disconnect();
  }

  private handleScroll = () => {
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
    const hasVerticalScroll = this.contentSize.height > this.viewport.height;
    const hasHorizontalScroll = this.contentSize.width > this.viewport.width;

    this.setThumbVisibility(this.thumb.y, hasVerticalScroll);
    this.setThumbVisibility(this.thumb.x, hasHorizontalScroll);
  }

  private setThumbVisibility(element: HTMLElement, visible: boolean) {
    element.style.opacity = visible ? "" : "0";
    element.style.pointerEvents = visible ? "" : "none";
  }

  private handleResize = (entries: ResizeObserverEntry[]) => {
    const entry = entries[0];
    if (!entry) return;

    this.viewport = {
      width: entry.contentRect.width,
      height: entry.contentRect.height,
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
    scrollBar.value = new ScrollBar(contentRef.value, innerRef.value, {
      x: thumbXRef.value,
      y: thumbYRef.value,
    });
  }
});

onUnmounted(() => {
  scrollBar.value?.clean();
});
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
    <div class="thumb thumb-y" ref="thumbYRef"></div>
    <div class="thumb thumb-x" ref="thumbXRef"></div>
    <div class="content" ref="contentRef">
      <div ref="innerRef" class="inner">
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

.container:hover {
  .thumb {
    opacity: 0.7;
    transition: opacity var(--duration-fast);
  }
}

.thumb {
  position: absolute;
  background-color: var(--neutral-600);
  opacity: 0;
  border-radius: var(--radius-full);
  transition: opacity var(--duration-fast) 1s;
  z-index: 1;
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
}

.content::-webkit-scrollbar {
  display: none;
}
</style>
