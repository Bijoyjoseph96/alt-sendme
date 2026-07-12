import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@/lib/platform-api'
import { IS_DESKTOP } from '@/lib/platform'
import {
	forgetPairedDevice,
	joinPairing,
	renamePairedDevice,
	setDeviceDisplayName,
	startPairingHost,
	stopPairingHost,
	type DeviceInfo,
	type PairedDevice,
} from '@/lib/pairing-api'
import {
	selectIsNodeReady,
	usePairingStore,
} from '@/store/pairing-store'

// Must match engine/protocol pairing::PAIRING_VOTE_TIMEOUT_SECS
const PAIRING_HOST_TTL_SECS = 120

export function usePairing() {
	const [pairingTicket, setPairingTicket] = useState<string | null>(null)
	const [hostExpiresIn, setHostExpiresIn] = useState<number | null>(null)
	const [isJoining, setIsJoining] = useState(false)
	const [isLoading, setIsLoading] = useState(false)
	// Incremented each time a peer joins while this device is hosting a
	// pairing window, so the UI can close the QR dialog and confirm success.
	const [hostPairedCount, setHostPairedCount] = useState(0)
	const pairingTicketRef = useRef<string | null>(null)

	const devices = usePairingStore((s) => s.pairedDevices)
	const thisDevice = usePairingStore((s) => s.thisDevice)
	const nodeStatus = usePairingStore((s) => s.nodeStatus)
	const isNodeReady = usePairingStore(selectIsNodeReady)
	const refreshDevices = usePairingStore((s) => s.refreshDevices)
	const refreshThisDevice = usePairingStore((s) => s.refreshThisDevice)

	useEffect(() => {
		pairingTicketRef.current = pairingTicket
	}, [pairingTicket])

	useEffect(() => {
		if (!IS_DESKTOP) return

		let disposed = false
		let unlistenPaired: (() => void) | undefined
		let unlistenExpired: (() => void) | undefined

		const setup = async () => {
			const pairedUnlisten = await listen('device-paired', () => {
				if (pairingTicketRef.current != null) {
					setPairingTicket(null)
					setHostExpiresIn(null)
					setHostPairedCount((count) => count + 1)
				}
			})
			if (disposed) {
				pairedUnlisten()
			} else {
				unlistenPaired = pairedUnlisten
			}

			const expiredUnlisten = await listen('pairing-host-expired', () => {
				setPairingTicket(null)
				setHostExpiresIn(null)
				toastManager.add({
					title: t('common:settings.devices.pairingHostExpired'),
					description: t('common:settings.devices.pairingHostExpiredDesc'),
					type: 'info',
				})
			})
			if (disposed) {
				expiredUnlisten()
			} else {
				unlistenExpired = expiredUnlisten
			}
		}

		void setup()

		return () => {
			disposed = true
			unlistenPaired?.()
			unlistenExpired?.()
		}
	}, [])

	useEffect(() => {
		if (hostExpiresIn == null || hostExpiresIn <= 0) return

		const timer = window.setInterval(() => {
			setHostExpiresIn((prev) => {
				if (prev == null || prev <= 1) {
					window.clearInterval(timer)
					return null
				}
				return prev - 1
			})
		}, 1000)

		return () => window.clearInterval(timer)
	}, [hostExpiresIn])

	const openHostPairing = useCallback(async () => {
		if (!IS_DESKTOP || !isNodeReady) return null
		setIsLoading(true)
		try {
			const ticket = await startPairingHost()
			setPairingTicket(ticket)
			setHostExpiresIn(PAIRING_HOST_TTL_SECS)
			return ticket
		} finally {
			setIsLoading(false)
		}
	}, [isNodeReady])

	const closeHostPairing = useCallback(async () => {
		setPairingTicket(null)
		setHostExpiresIn(null)
		await stopPairingHost()
	}, [])

	const join = useCallback(
		async (ticket: string) => {
			if (!IS_DESKTOP || !isNodeReady) return
			setIsJoining(true)
			try {
				await joinPairing(ticket.trim())
				await refreshDevices()
			} finally {
				setIsJoining(false)
			}
		},
		[isNodeReady, refreshDevices]
	)

	const forget = useCallback(
		async (endpointId: string) => {
			await forgetPairedDevice(endpointId)
			await refreshDevices()
		},
		[refreshDevices]
	)

	const renameThisDevice = useCallback(
		async (displayName: string) => {
			const updated = await setDeviceDisplayName(displayName)
			if (updated) await refreshThisDevice()
			return updated
		},
		[refreshThisDevice]
	)

	const renameDevice = useCallback(
		async (endpointId: string, displayName: string) => {
			const updated = await renamePairedDevice(endpointId, displayName)
			await refreshDevices()
			return updated
		},
		[refreshDevices]
	)

	return {
		devices,
		thisDevice,
		pairingTicket,
		hostExpiresIn,
		isJoining,
		isLoading,
		hostPairedCount,
		isNodeReady,
		nodeStatus,
		refreshDevices,
		refreshThisDevice,
		openHostPairing,
		closeHostPairing,
		join,
		forget,
		renameThisDevice,
		renameDevice,
	}
}

// Re-export types used by settings UI
export type { DeviceInfo, PairedDevice }
