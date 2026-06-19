import { useEffect, useMemo, useState } from 'react';
import { messagesByLocale } from './locales';
import { getInitialLocale, storeLocale } from './locale';
import type { Locale } from './types';

export function useI18n() {
  const [locale, setLocaleState] = useState<Locale>(() => getInitialLocale());

  const messages = messagesByLocale[locale];

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  const api = useMemo(
    () => ({
      locale,
      messages,
      setLocale(nextLocale: Locale) {
        setLocaleState(nextLocale);
        if (typeof window !== 'undefined') {
          storeLocale(window.localStorage, nextLocale);
          document.documentElement.lang = nextLocale;
        }
      },
    }),
    [locale, messages],
  );

  return api;
}
