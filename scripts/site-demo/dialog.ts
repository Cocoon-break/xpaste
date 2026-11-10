// 替换 @tauri-apps/plugin-dialog：演示页面不弹系统对话框。
export const open = async (_opts?: unknown): Promise<string | null> => null;

/** 演示页面不弹保存窗口（片段导出，M25 规则 11）：当作取消 */
export const save = async (_o?: unknown): Promise<string | null> => null;
