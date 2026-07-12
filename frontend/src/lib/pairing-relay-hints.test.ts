import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import {
	formatLastSeenAgo,
	isRecentlySeen,
	PAIRED_RECENTLY_SEEN_MS,
	relayPairingHintKey,
} from './pairing-relay-hints.js'

describe('pairing-relay-hints', () => {
	const t = (key: string, opts?: { count?: number }) => {
		if (key === 'common:settings.devices.lastSeenMinutes') {
			return `${opts?.count}m ago`
		}
		return key
	}

	it('maps relay modes to hint keys', () => {
		assert.equal(
			relayPairingHintKey('default'),
			'common:settings.devices.relayHintDefault'
		)
		assert.equal(
			relayPairingHintKey('custom'),
			'common:settings.devices.relayHintCustom'
		)
		assert.equal(
			relayPairingHintKey('disabled'),
			'common:settings.devices.relayHintDisabled'
		)
	})

	it('detects recent paired activity', () => {
		const now = 1_000_000
		assert.equal(isRecentlySeen(now - PAIRED_RECENTLY_SEEN_MS + 1, now), true)
		assert.equal(isRecentlySeen(now - PAIRED_RECENTLY_SEEN_MS - 1, now), false)
	})

	it('formats last seen labels', () => {
		const now = 1_000_000
		assert.equal(
			formatLastSeenAgo(now - 30_000, t, now),
			'common:settings.devices.lastSeenRecent'
		)
		assert.equal(formatLastSeenAgo(now - 120_000, t, now), '2m ago')
	})
})
