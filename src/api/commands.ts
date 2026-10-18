import { invoke } from '@tauri-apps/api/core';
import type { Bootstrap } from './types';

export const bootstrap = () => invoke<Bootstrap>('bootstrap');
