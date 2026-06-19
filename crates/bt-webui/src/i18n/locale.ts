import { locales, type Locale } from './types';

const STORAGE_KEY = 'bt-webui-locale';
const defaultLocale: Locale = 'en-US';

export function isLocale(value: string): value is Locale {
  return locales.includes(value as Locale);
}

export function resolveLocale(languages: readonly string[]): Locale {
  for (const language of languages) {
    const normalized = language.toLowerCase();
    if (normalized === 'zh-cn' || normalized === 'zh-hans' || normalized.startsWith('zh-hans-')) {
      return 'zh-CN';
    }
    if (normalized.startsWith('zh')) {
      return 'zh-CN';
    }
    if (normalized === 'en' || normalized.startsWith('en-')) {
      return 'en-US';
    }
  }
  return defaultLocale;
}

export function getStoredLocale(storage: Storage | undefined): Locale | undefined {
  if (!storage) {
    return undefined;
  }

  try {
    const value = storage.getItem(STORAGE_KEY);
    return value && isLocale(value) ? value : undefined;
  } catch {
    return undefined;
  }
}

export function storeLocale(storage: Storage | undefined, locale: Locale): void {
  if (!storage) {
    return;
  }

  try {
    storage.setItem(STORAGE_KEY, locale);
  } catch {
    // 用户禁用持久化时仍允许当前会话切换语言。
  }
}

export function getBrowserLanguages(navigatorRef: Navigator | undefined): readonly string[] {
  if (!navigatorRef) {
    return [];
  }
  return navigatorRef.languages.length > 0 ? navigatorRef.languages : [navigatorRef.language];
}

export function getInitialLocale(): Locale {
  if (typeof window === 'undefined') {
    return defaultLocale;
  }

  return getStoredLocale(window.localStorage) ?? resolveLocale(getBrowserLanguages(window.navigator));
}
