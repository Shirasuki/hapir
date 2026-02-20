import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { CanvasAddon } from '@xterm/addon-canvas'
import '@xterm/xterm/css/xterm.css'
import { ensureBuiltinFontLoaded, getFontProvider } from '@/lib/terminalFont'

const DEFAULT_FONT_SIZE = 13
const MIN_FONT_SIZE = 8
const MAX_FONT_SIZE = 28
const FONT_SIZE_KEY = 'hapir-terminal-font-size'

function loadFontSize(): number {
    try {
        const v = Number(localStorage.getItem(FONT_SIZE_KEY))
        if (v >= MIN_FONT_SIZE && v <= MAX_FONT_SIZE) return v
    } catch { /* ignore */ }
    return DEFAULT_FONT_SIZE
}

function saveFontSize(size: number): void {
    try {
        if (size === DEFAULT_FONT_SIZE) localStorage.removeItem(FONT_SIZE_KEY)
        else localStorage.setItem(FONT_SIZE_KEY, String(size))
    } catch { /* ignore */ }
}

function touchDist(a: Touch, b: Touch): number {
    const dx = a.clientX - b.clientX
    const dy = a.clientY - b.clientY
    return Math.sqrt(dx * dx + dy * dy)
}

function resolveThemeColors(): { background: string; foreground: string; selectionBackground: string } {
    const styles = getComputedStyle(document.documentElement)
    const background = styles.getPropertyValue('--app-bg').trim() || '#000000'
    const foreground = styles.getPropertyValue('--app-fg').trim() || '#ffffff'
    const selectionBackground = styles.getPropertyValue('--app-subtle-bg').trim() || 'rgba(255, 255, 255, 0.2)'
    return { background, foreground, selectionBackground }
}

export function TerminalView(props: {
    onMount?: (terminal: Terminal) => void
    onResize?: (cols: number, rows: number) => void
    className?: string
}) {
    const containerRef = useRef<HTMLDivElement | null>(null)
    const onMountRef = useRef(props.onMount)
    const onResizeRef = useRef(props.onResize)

    useEffect(() => {
        onMountRef.current = props.onMount
    }, [props.onMount])

    useEffect(() => {
        onResizeRef.current = props.onResize
    }, [props.onResize])

    useEffect(() => {
        const container = containerRef.current
        if (!container) return

        const abortController = new AbortController()
        const signal = abortController.signal

        const fontProvider = getFontProvider()
        const { background, foreground, selectionBackground } = resolveThemeColors()
        const initialFontSize = loadFontSize()
        const terminal = new Terminal({
            cursorBlink: true,
            fontFamily: fontProvider.getFontFamily(),
            fontSize: initialFontSize,
            theme: {
                background,
                foreground,
                cursor: foreground,
                selectionBackground
            },
            convertEol: true,
            customGlyphs: true
        })

        const fitAddon = new FitAddon()
        const webLinksAddon = new WebLinksAddon()
        const canvasAddon = new CanvasAddon()
        terminal.loadAddon(fitAddon)
        terminal.loadAddon(webLinksAddon)
        terminal.loadAddon(canvasAddon)
        terminal.open(container)

        const observer = new ResizeObserver(() => {
            requestAnimationFrame(() => {
                fitAddon.fit()
                onResizeRef.current?.(terminal.cols, terminal.rows)
            })
        })
        observer.observe(container)

        // Pinch-to-zoom & touch scroll
        let pinchStartDist = 0
        let pinchBaseFontSize = initialFontSize
        let scrollLastY = 0
        let scrollAccum = 0
        let isSingleTouch = false

        container.addEventListener('touchstart', (e) => {
            if (e.touches.length === 2) {
                isSingleTouch = false
                pinchStartDist = touchDist(e.touches[0], e.touches[1])
                pinchBaseFontSize = terminal.options.fontSize ?? initialFontSize
            } else if (e.touches.length === 1) {
                isSingleTouch = true
                scrollLastY = e.touches[0].clientY
                scrollAccum = 0
            }
        }, { passive: true, signal })

        container.addEventListener('touchmove', (e) => {
            if (e.touches.length === 2 && pinchStartDist !== 0) {
                e.preventDefault()
                const scale = touchDist(e.touches[0], e.touches[1]) / pinchStartDist
                const next = Math.round(Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, pinchBaseFontSize * scale)))
                if (next !== terminal.options.fontSize) {
                    terminal.options.fontSize = next
                    fitAddon.fit()
                    onResizeRef.current?.(terminal.cols, terminal.rows)
                }
                return
            }
            if (e.touches.length === 1 && isSingleTouch) {
                const y = e.touches[0].clientY
                const delta = scrollLastY - y
                scrollLastY = y
                const lineHeight = terminal.options.fontSize ?? initialFontSize
                scrollAccum += delta
                const lines = Math.trunc(scrollAccum / lineHeight)
                if (lines !== 0) {
                    scrollAccum -= lines * lineHeight
                    terminal.scrollLines(lines)
                }
            }
        }, { passive: false, signal })

        container.addEventListener('touchend', (e) => {
            if (e.touches.length < 2 && pinchStartDist !== 0) {
                pinchStartDist = 0
                saveFontSize(terminal.options.fontSize ?? initialFontSize)
            }
            if (e.touches.length === 0) {
                isSingleTouch = false
            }
        }, { passive: true, signal })

        const refreshFont = (forceRemeasure = false) => {
            if (signal.aborted) return
            const nextFamily = fontProvider.getFontFamily()

            if (forceRemeasure && terminal.options.fontFamily === nextFamily) {
                terminal.options.fontFamily = `${nextFamily}, "__hapir_font_refresh__"`
                requestAnimationFrame(() => {
                    if (signal.aborted) return
                    terminal.options.fontFamily = nextFamily
                    if (terminal.rows > 0) {
                        terminal.refresh(0, terminal.rows - 1)
                    }
                    fitAddon.fit()
                    onResizeRef.current?.(terminal.cols, terminal.rows)
                })
                return
            }

            terminal.options.fontFamily = nextFamily
            if (terminal.rows > 0) {
                terminal.refresh(0, terminal.rows - 1)
            }
            fitAddon.fit()
            onResizeRef.current?.(terminal.cols, terminal.rows)
        }

        void ensureBuiltinFontLoaded().then(loaded => {
            if (!loaded) return
            refreshFont(true)
        })

        // Cleanup on abort
        signal.addEventListener('abort', () => {
            observer.disconnect()
            fitAddon.dispose()
            webLinksAddon.dispose()
            canvasAddon.dispose()
            terminal.dispose()
        })

        requestAnimationFrame(() => {
            fitAddon.fit()
            onResizeRef.current?.(terminal.cols, terminal.rows)
        })
        onMountRef.current?.(terminal)

        return () => abortController.abort()
    }, [])

    return (
        <div
            ref={containerRef}
            className={`h-full w-full ${props.className ?? ''}`}
        />
    )
}
