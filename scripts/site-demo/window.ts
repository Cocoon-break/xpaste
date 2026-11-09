// 替换 @tauri-apps/api/window：演示页面里没有原生窗口，方法都是空操作。
const noop = async () => {};
const w = { label: 'demo', startDragging: noop, onMoved: async () => () => {} };
export const getCurrentWindow = () => w;
export const currentMonitor = async () => null;
export const primaryMonitor = async () => null;
