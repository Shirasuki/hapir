import { useCallback, useEffect, useLayoutEffect, useState } from 'react'
import { isBrowser, safeGetItem, safeSetItem, safeRemoveItem } from '@/lib/utils'

export type FontScale = 0.8 | 0.9 | 1 | 1.1 | 1.2

export function getFontScaleOptions(): ReadonlyArray<{ value: FontScale; label: string }> {
    return [
        { value: 0.8, label: '80%' },
        { value: 0.9, label: '90%' },
        { value: 1, label: '100%' },
        { value: 1.1, label: '110%' },
        { value: 1.2, label: '120%' },
    ]
}

const STORAGE_KEY = 'hapir-font-scale'

const useIsomorphicLayoutEffect = isBrowser() ? useLayoutEffect : useEffect

function parseFontScale(raw: string | null): FontScale {
    const value = Number(raw)
    if (value === 0.8 || value === 0.9 || value === 1 || value === 1.1 || value === 1.2) {
        return value
    }
    return 1
}

function applyFontScale(scale: FontScale): void {
    if (!isBrowser()) {
        return
    }
    document.documentElement.style.setProperty('--app-font-scale', String(scale))
}

function getInitialFontScale(): FontScale {
    return parseFontScale(safeGetItem(STORAGE_KEY))
}

export function initializeFontScale(): void {
    applyFontScale(getInitialFontScale())
}

export function useFontScale(): { fontScale: FontScale; setFontScale: (scale: FontScale) => void } {
    const [fontScale, setFontScaleState] = useState<FontScale>(getInitialFontScale)

    useIsomorphicLayoutEffect(() => {
        applyFontScale(fontScale)
    }, [fontScale])

    useEffect(() => {
        if (!isBrowser()) {
            return
        }

        const onStorage = (event: StorageEvent) => {
            if (event.key !== STORAGE_KEY) {
                return
            }
            const nextScale = parseFontScale(event.newValue)
            setFontScaleState(nextScale)
        }

        window.addEventListener('storage', onStorage)
        return () => window.removeEventListener('storage', onStorage)
    }, [])

    const setFontScale = useCallback((scale: FontScale) => {
        setFontScaleState(scale)

        if (scale === 1) {
            safeRemoveItem(STORAGE_KEY)
        } else {
            safeSetItem(STORAGE_KEY, String(scale))
        }
    }, [])

    return { fontScale, setFontScale }
}
