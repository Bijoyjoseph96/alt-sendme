import { create } from 'zustand'
import type { PairedInvitePayload } from '@/lib/pairing-api'

type PairedInviteState = {
	invite: PairedInvitePayload | null
	setInvite: (invite: PairedInvitePayload | null) => void
}

export const usePairedInviteStore = create<PairedInviteState>((set, get) => ({
	invite: null,
	setInvite: (invite) => {
		if (
			invite &&
			get().invite?.blob_ticket === invite.blob_ticket
		) {
			return
		}
		set({ invite })
	},
}))
