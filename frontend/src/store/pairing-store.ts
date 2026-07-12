import { create } from 'zustand'
import { listen } from '@/lib/platform-api'
import { IS_DESKTOP } from '@/lib/platform'
import { getRelayConfigArg } from '@/lib/relay'
import {
	getDeviceInfo,
	getNodeStatus,
	listPairedDevices,
	reconfigureNodeRelay,
	type DeviceInfo,
	type NodeStatus,
	type PairedDevice,
} from '@/lib/pairing-api'

type PairingState = {
	nodeStatus: NodeStatus
	pairedDevices: PairedDevice[]
	thisDevice: DeviceInfo | null
	/** Ensures listeners/refresh run once for the whole app. */
	bootstrapped: boolean
	refreshNodeStatus: () => Promise<void>
	refreshDevices: () => Promise<void>
	refreshThisDevice: () => Promise<void>
	bootstrap: () => void
}

let unlistenDevicePaired: (() => void) | undefined
let unlistenHostExpired: (() => void) | undefined
/** Set once relay settings sync to the node succeeds; cleared on failure for retry. */
let nodeRelaySynced = false

async function syncNodeRelayIfNeeded(
	getState: () => PairingState
): Promise<void> {
	if (!IS_DESKTOP || nodeRelaySynced) return
	if (getState().nodeStatus.status !== 'ready') return
	try {
		await reconfigureNodeRelay(getRelayConfigArg())
		nodeRelaySynced = true
	} catch (error) {
		console.warn('Failed to sync node relay:', error)
	}
}

export const usePairingStore = create<PairingState>((set, get) => ({
	nodeStatus: { status: 'unavailable' },
	pairedDevices: [],
	thisDevice: null,
	bootstrapped: false,

	refreshNodeStatus: async () => {
		if (!IS_DESKTOP) {
			set({ nodeStatus: { status: 'unavailable', reason: 'desktop_only' } })
			return
		}
		try {
			const nodeStatus = await getNodeStatus()
			const wasReady = get().nodeStatus.status === 'ready'
			set({ nodeStatus })
			if (nodeStatus.status === 'ready') {
				if (!wasReady) {
					await Promise.all([
						get().refreshDevices(),
						get().refreshThisDevice(),
					])
				}
				await syncNodeRelayIfNeeded(get)
			}
		} catch (error) {
			console.error('Failed to get node status:', error)
			set({
				nodeStatus: { status: 'unavailable', reason: String(error) },
			})
		}
	},

	refreshDevices: async () => {
		if (!IS_DESKTOP) {
			set({ pairedDevices: [] })
			return
		}
		const ready = get().nodeStatus.status === 'ready'
		if (!ready) {
			set({ pairedDevices: [] })
			return
		}
		try {
			set({ pairedDevices: await listPairedDevices() })
		} catch (error) {
			console.error('Failed to list paired devices:', error)
		}
	},

	refreshThisDevice: async () => {
		if (!IS_DESKTOP) {
			set({ thisDevice: null })
			return
		}
		if (get().nodeStatus.status !== 'ready') {
			set({ thisDevice: null })
			return
		}
		try {
			set({ thisDevice: await getDeviceInfo() })
		} catch (error) {
			console.error('Failed to load this device:', error)
		}
	},

	bootstrap: () => {
		if (get().bootstrapped || !IS_DESKTOP) return
		set({ bootstrapped: true })

		const run = async () => {
			await get().refreshNodeStatus()
			if (get().nodeStatus.status === 'ready') {
				await Promise.all([get().refreshDevices(), get().refreshThisDevice()])
			}

			unlistenDevicePaired = await listen('device-paired', () => {
				void get().refreshNodeStatus().then(() =>
					Promise.all([get().refreshDevices(), get().refreshThisDevice()])
				)
			})
			unlistenHostExpired = await listen('pairing-host-expired', () => {
				void get().refreshNodeStatus()
			})
		}

		void run()
	},
}))

/** Tear down listeners (tests / hot reload). Not required for normal app life. */
export function disposePairingStoreListeners() {
	unlistenDevicePaired?.()
	unlistenHostExpired?.()
	unlistenDevicePaired = undefined
	unlistenHostExpired = undefined
	nodeRelaySynced = false
	usePairingStore.setState({ bootstrapped: false })
}

/** Call after relay reconfigure succeeds outside bootstrap (e.g. settings guard). */
export function markNodeRelaySynced() {
	nodeRelaySynced = true
}

export function selectIsNodeReady(state: PairingState): boolean {
	return IS_DESKTOP && state.nodeStatus.status === 'ready'
}
