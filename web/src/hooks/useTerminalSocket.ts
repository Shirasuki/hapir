import { useCallback, useEffect, useRef, useState } from 'react'

type TerminalConnectionState =
    | { status: 'idle' }
    | { status: 'connecting' }
    | { status: 'connected' }
    | { status: 'error'; error: string }

type UseTerminalSocketOptions = {
    baseUrl: string
    token: string
    sessionId: string
    terminalId: string
}

type TerminalReadyPayload = {
    terminalId: string
}

type TerminalOutputPayload = {
    terminalId: string
    data: string
}

type TerminalExitPayload = {
    terminalId: string
    code: number | null
    signal: string | null
}

type TerminalErrorPayload = {
    terminalId: string
    message: string
}

type WsMessage = {
    event: string
    data?: unknown
}

const RECONNECT_BASE = 1000
const RECONNECT_MAX = 5000

export function useTerminalSocket(options: UseTerminalSocketOptions): {
    state: TerminalConnectionState
    connect: (cols: number, rows: number) => void
    write: (data: string) => void
    resize: (cols: number, rows: number) => void
    disconnect: () => void
    onOutput: (handler: (data: string) => void) => void
    onExit: (handler: (code: number | null, signal: string | null) => void) => void
} {
    const [state, setState] = useState<TerminalConnectionState>({ status: 'idle' })
    const wsRef = useRef<WebSocket | null>(null)
    const outputHandlerRef = useRef<(data: string) => void>(() => {})
    const exitHandlerRef = useRef<(code: number | null, signal: string | null) => void>(() => {})
    const sessionIdRef = useRef(options.sessionId)
    const terminalIdRef = useRef(options.terminalId)
    const tokenRef = useRef(options.token)
    const baseUrlRef = useRef(options.baseUrl)
    const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null)
    const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
    const reconnectAttemptRef = useRef(0)
    const intentionalCloseRef = useRef(false)

    useEffect(() => {
        sessionIdRef.current = options.sessionId
        terminalIdRef.current = options.terminalId
        baseUrlRef.current = options.baseUrl
    }, [options.sessionId, options.terminalId, options.baseUrl])

    useEffect(() => {
        tokenRef.current = options.token
        const ws = wsRef.current
        if (!ws) return
        if (!options.token) {
            intentionalCloseRef.current = true
            ws.close()
            return
        }
        // Token changed — reconnect with new token
        intentionalCloseRef.current = true
        ws.close()
        wsRef.current = null
        const size = lastSizeRef.current
        if (size) {
            // Re-trigger connect with last known size
            scheduleReconnect(0)
        }
    }, [options.token])

    const isCurrentTerminal = useCallback((terminalId: string) => terminalId === terminalIdRef.current, [])

    const send = useCallback((event: string, data: Record<string, unknown>) => {
        const ws = wsRef.current
        if (!ws || ws.readyState !== WebSocket.OPEN) return
        ws.send(JSON.stringify({ event, data }))
    }, [])

    const emitCreate = useCallback((size: { cols: number; rows: number }) => {
        send('terminal:create', {
            sessionId: sessionIdRef.current,
            terminalId: terminalIdRef.current,
            cols: size.cols,
            rows: size.rows,
        })
    }, [send])

    const setErrorState = useCallback((message: string) => {
        setState({ status: 'error', error: message })
    }, [])

    const clearReconnectTimer = useCallback(() => {
        if (reconnectTimerRef.current) {
            clearTimeout(reconnectTimerRef.current)
            reconnectTimerRef.current = null
        }
    }, [])

    const scheduleReconnect = useCallback((delayOverride?: number) => {
        clearReconnectTimer()
        const attempt = reconnectAttemptRef.current
        const delay = delayOverride ?? Math.min(RECONNECT_BASE * Math.pow(2, attempt), RECONNECT_MAX)
        reconnectAttemptRef.current = attempt + 1
        reconnectTimerRef.current = setTimeout(() => {
            const size = lastSizeRef.current
            if (size) {
                createWs(size.cols, size.rows)
            }
        }, delay)
    }, [])

    const handleMessage = useCallback((e: MessageEvent) => {
        let msg: WsMessage
        try {
            msg = JSON.parse(e.data)
        } catch {
            return
        }
        const data = msg.data as Record<string, unknown> | undefined
        if (!data) return

        switch (msg.event) {
            case 'terminal:ready': {
                const payload = data as unknown as TerminalReadyPayload
                if (!isCurrentTerminal(payload.terminalId)) return
                setState({ status: 'connected' })
                break
            }
            case 'terminal:output': {
                const payload = data as unknown as TerminalOutputPayload
                if (!isCurrentTerminal(payload.terminalId)) return
                outputHandlerRef.current(payload.data)
                break
            }
            case 'terminal:exit': {
                const payload = data as unknown as TerminalExitPayload
                if (!isCurrentTerminal(payload.terminalId)) return
                exitHandlerRef.current(payload.code, payload.signal)
                setErrorState('Terminal exited.')
                break
            }
            case 'terminal:error': {
                const payload = data as unknown as TerminalErrorPayload
                if (!isCurrentTerminal(payload.terminalId)) return
                setErrorState(payload.message)
                break
            }
        }
    }, [isCurrentTerminal, setErrorState])

    const createWs = useCallback((cols: number, rows: number) => {
        if (wsRef.current) {
            intentionalCloseRef.current = true
            wsRef.current.close()
            wsRef.current = null
        }

        const token = tokenRef.current
        if (!token || !sessionIdRef.current || !terminalIdRef.current) {
            setErrorState('Missing terminal credentials.')
            return
        }

        intentionalCloseRef.current = false
        setState({ status: 'connecting' })

        const base = baseUrlRef.current.replace(/^http/, 'ws')
        const ws = new WebSocket(`${base}/ws/terminal?token=${encodeURIComponent(token)}`)
        wsRef.current = ws

        ws.onopen = () => {
            reconnectAttemptRef.current = 0
            const size = lastSizeRef.current ?? { cols, rows }
            setState({ status: 'connecting' })
            emitCreate(size)
        }

        ws.onmessage = handleMessage

        ws.onerror = () => {
            setErrorState('Connection error')
        }

        ws.onclose = () => {
            wsRef.current = null
            if (intentionalCloseRef.current) {
                setState({ status: 'idle' })
                return
            }
            setErrorState('Disconnected')
            scheduleReconnect()
        }
    }, [emitCreate, handleMessage, setErrorState, scheduleReconnect])

    const connect = useCallback((cols: number, rows: number) => {
        lastSizeRef.current = { cols, rows }
        clearReconnectTimer()
        reconnectAttemptRef.current = 0

        const ws = wsRef.current
        if (ws && ws.readyState === WebSocket.OPEN) {
            emitCreate({ cols, rows })
            return
        }

        createWs(cols, rows)
    }, [emitCreate, createWs, clearReconnectTimer])

    const write = useCallback((data: string) => {
        send('terminal:write', { terminalId: terminalIdRef.current, data })
    }, [send])

    const resize = useCallback((cols: number, rows: number) => {
        lastSizeRef.current = { cols, rows }
        send('terminal:resize', { terminalId: terminalIdRef.current, cols, rows })
    }, [send])

    const disconnect = useCallback(() => {
        clearReconnectTimer()
        intentionalCloseRef.current = true
        const ws = wsRef.current
        if (ws) {
            ws.close()
            wsRef.current = null
        }
        setState({ status: 'idle' })
    }, [clearReconnectTimer])

    const onOutput = useCallback((handler: (data: string) => void) => {
        outputHandlerRef.current = handler
    }, [])

    const onExit = useCallback((handler: (code: number | null, signal: string | null) => void) => {
        exitHandlerRef.current = handler
    }, [])

    return { state, connect, write, resize, disconnect, onOutput, onExit }
}
