import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]): string {
    return twMerge(clsx(inputs))
}

export function isBrowser(): boolean {
    return typeof window !== 'undefined' && typeof document !== 'undefined'
}

export function safeGetItem(key: string): string | null {
    if (!isBrowser()) return null
    try {
        return localStorage.getItem(key)
    } catch {
        return null
    }
}

export function safeSetItem(key: string, value: string): void {
    if (!isBrowser()) return
    try {
        localStorage.setItem(key, value)
    } catch {
        // Ignore storage errors
    }
}

export function safeRemoveItem(key: string): void {
    if (!isBrowser()) return
    try {
        localStorage.removeItem(key)
    } catch {
        // Ignore storage errors
    }
}

/**
 * Decode base64 string to UTF-8 text
 */
export function decodeBase64(value: string): { text: string; ok: boolean } {
    try {
        const binaryString = atob(value)
        const bytes = Uint8Array.from(binaryString, (char) => char.charCodeAt(0))
        const text = new TextDecoder('utf-8').decode(bytes)
        return { text, ok: true }
    } catch {
        return { text: '', ok: false }
    }
}

/**
 * Encode UTF-8 text to base64 string
 */
export function encodeBase64(value: string): string {
    const bytes = new TextEncoder().encode(value)
    const binaryString = Array.from(bytes, (byte) => String.fromCharCode(byte)).join('')
    return btoa(binaryString)
}

