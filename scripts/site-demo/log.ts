// 替换 @tauri-apps/plugin-log：演示页面不写日志。
export const attachConsole = async () => () => {};
export const info = async (_m: string) => {};
export const error = async (_m: string) => {};
