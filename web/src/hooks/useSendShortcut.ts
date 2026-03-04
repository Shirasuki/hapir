import { useCallback, useEffect, useState } from 'react'
import { isBrowser, safeGetItem, safeSetItem, safeRemoveItem } from '@/lib/utils'

export type SendShortcut = 'auto' | 'enter' | 'shift-enter' | 'cmd-enter'

export function getSendShortcutOptions(): ReadonlyArray<{ value: SendShortcut; labelKey: string }> {
    return [
        { value: 'auto', labelKey: 'settings.keyboard.sendShortcut.auto' },
        { value: 'enter', labelKey: 'settings.keyboard.sendShortcut.enter' },
        { value: 'shift-enter', labelKey: 'settings.keyboard.sendShortcut.shiftEnter' },
        { value: 'cmd-enter', labelKey: 'settings.keyboard.sendShortcut.cmdEnter' },
    ]
}

const STORAGE_KEY = 'hapir-send-shortcut'

function parseSendShortcut(raw: string | null): SendShortcut {
    if (raw === 'auto' || raw === 'enter' || raw === 'shift-enter' || raw === 'cmd-enter') {
        return raw
    }
    return 'auto'
}

function getInitialSendShortcut(): SendShortcut {
    return parseSendShortcut(safeGetItem(STORAGE_KEY))
}

export function useSendShortcut(): { sendShortcut: SendShortcut; setSendShortcut: (shortcut: SendShortcut) => void } {
    const [sendShortcut, setSendShortcutState] = useState<SendShortcut>(getInitialSendShortcut)

    useEffect(() => {
        if (!isBrowser()) return

        const onStorage = (event: StorageEvent) => {
            if (event.key !== STORAGE_KEY) return
            setSendShortcutState(parseSendShortcut(event.newValue))
        }

        window.addEventListener('storage', onStorage)
        return () => window.removeEventListener('storage', onStorage)
    }, [])

    const setSendShortcut = useCallback((shortcut: SendShortcut) => {
        setSendShortcutState(shortcut)
        if (shortcut === 'auto') {
            safeRemoveItem(STORAGE_KEY)
        } else {
            safeSetItem(STORAGE_KEY, shortcut)
        }
    }, [])

    return { sendShortcut, setSendShortcut }
}
