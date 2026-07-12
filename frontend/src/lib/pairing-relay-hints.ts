import type { RelayMode } from './relay-config'

/** Paired control-plane activity within this window is treated as "recent". */
export const PAIRED_RECENTLY_SEEN_MS = 5 * 60 * 1000

type TranslateFn = (
	key: string,
	options?: { count?: number }
) => string

export function relayPairingHintKey(relayMode: RelayMode): string {
	switch (relayMode) {
		case 'disabled':
			return 'common:settings.devices.relayHintDisabled'
		case 'custom':
			return 'common:settings.devices.relayHintCustom'
		default:
			return 'common:settings.devices.relayHintDefault'
	}
}

export function isRecentlySeen(
	lastSeenAt: number,
	nowMs: number = Date.now()
): boolean {
	return lastSeenAt > 0 && nowMs - lastSeenAt < PAIRED_RECENTLY_SEEN_MS
}

export function formatLastSeenAgo(
	lastSeenAt: number,
	t: TranslateFn,
	nowMs: number = Date.now()
): string | null {
	if (!lastSeenAt || lastSeenAt <= 0) return null

	const diffSec = Math.max(0, Math.floor((nowMs - lastSeenAt) / 1000))
	if (diffSec < 60) {
		return t('common:settings.devices.lastSeenRecent')
	}

	const diffMin = Math.floor(diffSec / 60)
	if (diffMin < 60) {
		return t('common:settings.devices.lastSeenMinutes', { count: diffMin })
	}

	const diffHr = Math.floor(diffMin / 60)
	if (diffHr < 48) {
		return t('common:settings.devices.lastSeenHours', { count: diffHr })
	}

	const diffDay = Math.floor(diffHr / 24)
	return t('common:settings.devices.lastSeenDays', { count: diffDay })
}
