import { useTranslation } from '@/i18n'
import { relayPairingHintKey } from '@/lib/pairing-relay-hints'
import { useAppSettingStore } from '@/store/app-setting'
import { cn } from '@/lib/utils'
import { Info } from 'lucide-react'

type RelayPairingCalloutProps = {
	className?: string
}

/** Mode-aware hint for how relay settings affect pairing. */
export function RelayPairingCallout({ className }: RelayPairingCalloutProps) {
	const { t } = useTranslation()
	const relayMode = useAppSettingStore((s) => s.relayMode)

	return (
		<div
			className={cn(
				'flex items-start gap-2 rounded-md border border-border/60 bg-muted/40 px-3 py-2 text-xs text-muted-foreground',
				className
			)}
		>
			<Info className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden />
			<span>{t(relayPairingHintKey(relayMode))}</span>
		</div>
	)
}
