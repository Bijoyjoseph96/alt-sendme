import { useCallback } from 'react'
import {
	selectIsNodeReady,
	usePairingStore,
} from '@/store/pairing-store'
import type { NodeStatus } from '@/lib/pairing-api'

/** Thin read API over the shared pairing store (single IPC source). */
export function useNodeCapability() {
	const nodeStatus = usePairingStore((s) => s.nodeStatus)
	const isNodeReady = usePairingStore(selectIsNodeReady)
	const refreshNodeStatus = usePairingStore((s) => s.refreshNodeStatus)

	return {
		nodeStatus: nodeStatus as NodeStatus,
		isNodeReady,
		refreshNodeStatus: useCallback(() => refreshNodeStatus(), [refreshNodeStatus]),
	}
}
