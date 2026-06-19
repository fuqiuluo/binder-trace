import { enUS } from './en-US';
import { zhCN } from './zh-CN';
import type { Locale, LocaleOption, Messages } from '../types';

export const messagesByLocale: Record<Locale, Messages> = {
  'en-US': enUS,
  'zh-CN': zhCN,
};

export const localeOptions: LocaleOption[] = [
  { locale: 'en-US', label: enUS.language.english, nativeLabel: 'English' },
  { locale: 'zh-CN', label: zhCN.language.chinese, nativeLabel: '简体中文' },
];
