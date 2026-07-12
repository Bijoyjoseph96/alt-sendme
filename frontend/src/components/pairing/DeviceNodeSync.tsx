import { useEffect, useRef } from 'react'
import { listen } from '@/lib/platform-api'
import { IS_DESKTOP } from '@/lib/platform'
import type { PairedInvitePayload } from '@/lib/pairing-api'
import { usePairedInviteStore } from '@/store/paired-invite-store'
import { usePairingStore } from '@/store/pairing-store'

/**
 * Bootstraps shared pairing state once and listens for paired invites globally.
 * Mounted from RootLayout on desktop.
 */
export function DeviceNodeSync() {
	const setInvite = usePairedInviteStore((s) => s.setInvite)
	const started = useRef(false)

	useEffect(() => {
		if (!IS_DESKTOP || started.current) return
		started.current = true
		usePairingStore.getState().bootstrap()
	}, [])

	useEffect(() => {
		if (!IS_DESKTOP) return

		let disposed = false
		let unlistenInvite: (() => void) | undefined

		const setup = async () => {
			const inviteUnlisten = await listen(
				'paired-invite-received',
				(event: { payload: unknown }) => {
					try {
						const payload = JSON.parse(
							String(event.payload)
						) as PairedInvitePayload
						setInvite(payload)
					} catch (error) {
						console.error('Failed to parse paired invite:', error)
					}
				}
			)
			if (disposed) {
				inviteUnlisten()
			} else {
				unlistenInvite = inviteUnlisten
			}
		}

		void setup()

		return () => {
			disposed = true
			unlistenInvite?.()
		}
	}, [setInvite])

	return null
}
