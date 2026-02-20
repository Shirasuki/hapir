function fallbackCopy(text: string): boolean {
    const prev = document.activeElement as HTMLElement | null
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.style.position = 'fixed'
    textarea.style.left = '-9999px'
    textarea.style.top = '-9999px'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.focus()
    textarea.select()
    let ok = false
    try {
        ok = document.execCommand('copy')
    } catch {
        // ignore
    }
    document.body.removeChild(textarea)
    prev?.focus()
    return ok
}

export async function safeCopyToClipboard(text: string): Promise<void> {
    if (navigator.clipboard?.writeText) {
        try {
            await navigator.clipboard.writeText(text)
            return
        } catch {
            // fall through to legacy fallback
        }
    }
    if (fallbackCopy(text)) {
        return
    }
    throw new Error('Copy to clipboard failed')
}
