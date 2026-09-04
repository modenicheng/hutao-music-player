// mock API 层统一出口：页面只 import 这里，未来切真实后端时无需改调用方。
export * from "./types";
export { api } from "./client";
export { avatarUrl, coverUrl } from "./covers";
