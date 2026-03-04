import { useCallback, useEffect, useLayoutEffect, useState } from 'react'
import { isBrowser, safeGetItem, safeSetItem, safeRemoveItem } from '@/lib/utils'

export type ColorTheme = 'default' | 'claude'

export function getColorThemeOptions(): ReadonlyArray<{ value: ColorTheme; labelKey: string }> {
    return [
        { value: 'default', labelKey: 'settings.display.theme.default' },
        { value: 'claude', labelKey: 'settings.display.theme.claude' },
    ]
}

const STORAGE_KEY = 'hapir-color-theme'

const useIsomorphicLayoutEffect = isBrowser() ? useLayoutEffect : useEffect

function parseColorTheme(raw: string | null): ColorTheme {
    if (raw === 'claude') return 'claude'
    return 'default'
}

export function applyColorTheme(theme: ColorTheme): void {
    if (!isBrowser()) return
    if (theme === 'default') {
        document.documentElement.removeAttribute('data-color-theme')
    } else {
        document.documentElement.setAttribute('data-color-theme', theme)
    }
}

function getInitialColorTheme(): ColorTheme {
    return parseColorTheme(safeGetItem(STORAGE_KEY))
}

export function useColorTheme(): { colorTheme: ColorTheme; setColorTheme: (theme: ColorTheme) => void } {
    const [colorTheme, setColorThemeState] = useState<ColorTheme>(getInitialColorTheme)

    useIsomorphicLayoutEffect(() => {
        applyColorTheme(colorTheme)
    }, [colorTheme])

    useEffect(() => {
        if (!isBrowser()) return

        const onStorage = (event: StorageEvent) => {
            if (event.key !== STORAGE_KEY) return
            setColorThemeState(parseColorTheme(event.newValue))
        }

        window.addEventListener('storage', onStorage)
        return () => window.removeEventListener('storage', onStorage)
    }, [])

    const setColorTheme = useCallback((theme: ColorTheme) => {
        setColorThemeState(theme)
        if (theme === 'default') {
            safeRemoveItem(STORAGE_KEY)
        } else {
            safeSetItem(STORAGE_KEY, theme)
        }
    }, [])

    return { colorTheme, setColorTheme }
}
