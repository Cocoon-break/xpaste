// 首次启动引导（M18）的步骤顺序：辅助功能只在 macOS 需要
export type Step = 'hotkey' | 'accessibility' | 'autostart';

export const onboardSteps = (os: 'macos' | 'windows'): Step[] =>
  os === 'macos' ? ['hotkey', 'accessibility', 'autostart'] : ['hotkey', 'autostart'];

export const nextStep = (steps: Step[], cur: Step): Step | null => steps[steps.indexOf(cur) + 1] ?? null;
