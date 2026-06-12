<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import Checkbox from '$lib/components/common/Checkbox.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import TextArea from '$lib/components/common/TextArea.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { displayTz } from '$lib/stores/timezone.svelte';
	import { formatAbsolute } from '$lib/utils/format-time';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { FlowListItem, Schedule, ScheduleInput } from '$lib/types';

	interface Props {
		open: boolean;
		/** Existing schedule when editing; null/undefined when creating. */
		schedule?: Schedule | null;
		/** Pre-selected flow (stable id) when creating from a flow ("Schedule" button). */
		defaultFlowId?: string;
		/** Flows to choose from in the picker. */
		flows: FlowListItem[];
		onClose: () => void;
		onSaved: () => void;
	}

	let { open, schedule = null, defaultFlowId, flows, onClose, onSaved }: Props = $props();

	const CRON_PRESETS = [
		{ label: 'Preset…', value: '' },
		{ label: 'Every 10 seconds', value: '*/10 * * * * *' },
		{ label: 'Every 30 seconds', value: '*/30 * * * * *' },
		{ label: 'Every minute', value: '* * * * *' },
		{ label: 'Every 5 minutes', value: '*/5 * * * *' },
		{ label: 'Hourly', value: '0 * * * *' },
		{ label: 'Daily 00:00', value: '0 0 * * *' },
		{ label: 'Weekdays 09:00', value: '0 9 * * 1-5' },
		{ label: 'Weekly (Sun 00:00)', value: '0 0 * * 0' }
	];

	const TIMEZONES = [
		'UTC',
		'Asia/Taipei',
		'Asia/Tokyo',
		'Asia/Shanghai',
		'Asia/Kolkata',
		'Europe/London',
		'Europe/Berlin',
		'America/New_York',
		'America/Los_Angeles'
	];

	const isEdit = $derived(!!schedule);

	// Form state, (re)seeded whenever the modal opens for a new target.
	let name = $state('');
	let flowId = $state('');
	let cronExpr = $state('0 0 * * *');
	let timezone = $state('UTC');
	let argsText = $state('{}');
	let enabled = $state(true);
	let catchup = $state(false);
	let maxActive = $state('');
	let saving = $state(false);

	let seededFor = '';
	$effect(() => {
		if (!open) return;
		const key = schedule?.id ?? `new:${defaultFlowId ?? ''}`;
		if (key === seededFor) return;
		seededFor = key;
		if (schedule) {
			name = schedule.name;
			flowId = schedule.flow_id;
			cronExpr = schedule.cron_expr;
			timezone = schedule.timezone;
			argsText = JSON.stringify(schedule.args ?? {}, null, 2);
			enabled = schedule.enabled;
			catchup = schedule.catchup;
			maxActive = schedule.max_active_runs != null ? String(schedule.max_active_runs) : '';
		} else {
			name = '';
			flowId = defaultFlowId ?? flows[0]?.flow_id ?? '';
			cronExpr = '0 0 * * *';
			timezone = 'UTC';
			argsText = '{}';
			enabled = true;
			catchup = false;
			maxActive = '';
		}
	});

	// Live "next runs" preview from the backend (shared cron parser).
	let preview = $state<string[]>([]);
	let previewError = $state('');
	let previewGen = 0;
	$effect(() => {
		const cron = cronExpr.trim();
		const tz = timezone;
		const ws = workspace.id;
		if (!cron || !ws) {
			preview = [];
			previewError = '';
			return;
		}
		// Generation guard: a slow earlier request must not overwrite a newer one.
		const gen = ++previewGen;
		const handle = setTimeout(() => {
			api
				.forWorkspace(ws)
				.previewSchedule(cron, tz, 5)
				.then((r) => {
					if (gen !== previewGen) return;
					preview = r.next;
					previewError = '';
				})
				.catch((e) => {
					if (gen !== previewGen) return;
					preview = [];
					previewError = e instanceof ApiClientError ? e.body || e.message : String(e);
				});
		}, 300);
		return () => clearTimeout(handle);
	});

	const flowOptions = $derived(flows.map((f) => ({ label: f.path, value: f.flow_id })));
	const tzOptions = $derived(
		(TIMEZONES.includes(timezone) ? TIMEZONES : [timezone, ...TIMEZONES]).map((t) => ({
			label: t,
			value: t
		}))
	);

	function fmt(ts: string): string {
		return formatAbsolute(ts, displayTz.value);
	}

	async function save(): Promise<void> {
		if (!name.trim()) {
			toastError('Name is required');
			return;
		}
		if (!flowId) {
			toastError('Pick a flow');
			return;
		}
		let args: unknown;
		try {
			args = JSON.parse(argsText.trim() || '{}');
		} catch {
			toastError('Args must be valid JSON');
			return;
		}
		const max = maxActive.trim() === '' ? null : Number(maxActive);
		if (max !== null && (!Number.isInteger(max) || max < 1)) {
			toastError('Max active runs must be a positive integer (or empty for unlimited)');
			return;
		}
		const body: ScheduleInput = {
			name: name.trim(),
			flow_id: flowId,
			cron_expr: cronExpr.trim(),
			timezone,
			args,
			enabled,
			catchup,
			max_active_runs: max
		};
		saving = true;
		try {
			const ws = workspace.id;
			if (schedule) await api.forWorkspace(ws).updateSchedule(schedule.id, body);
			else await api.forWorkspace(ws).createSchedule(body);
			toastSuccess(schedule ? 'Schedule updated' : 'Schedule created');
			onSaved();
		} catch (e) {
			toastError(e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e));
		} finally {
			saving = false;
		}
	}
</script>

<Modal {open} title={isEdit ? 'Edit schedule' : 'New schedule'}>
	<div class="flex flex-col gap-3">
		<TextInput label="Name" bind:value={name} placeholder="nightly-etl" />

		<Select
			label="Flow"
			options={flowOptions}
			value={flowId}
			disabled={!!defaultFlowId && !isEdit}
			onchange={(v) => (flowId = v)}
		/>

		<div class="flex flex-col gap-1">
			<TextInput label="Cron expression" bind:value={cronExpr} mono placeholder="0 2 * * 1-5" />
			<Select
				ariaLabel="Cron preset"
				options={CRON_PRESETS}
				value=""
				compact
				onchange={(v) => v && (cronExpr = v)}
			/>
			<span class="text-[11px] text-text-tertiary">
				5 fields (min · hour · day · month · weekday), or 6 with a leading seconds field for
				sub-minute (e.g. <code>*/10 * * * * *</code>). Minimum interval: 10 seconds.
			</span>
		</div>

		<Select label="Timezone" options={tzOptions} value={timezone} onchange={(v) => (timezone = v)} />

		<div class="rounded-md border border-border bg-surface-alt p-2 text-xs">
			<span class="text-text-tertiary">Next runs</span>
			{#if previewError}
				<p class="mt-1 text-error">{previewError}</p>
			{:else if preview.length === 0}
				<p class="mt-1 text-text-tertiary">—</p>
			{:else}
				<ul class="mt-1 flex flex-col gap-0.5 font-mono text-text-secondary">
					{#each preview as t (t)}
						<li>{fmt(t)}</li>
					{/each}
				</ul>
			{/if}
		</div>

		<TextArea label="Args (JSON → flow.input)" bind:value={argsText} mono rows={3} />

		<Checkbox label="Enabled" bind:checked={enabled} />

		<Checkbox
			label="Catch up missed runs"
			description="Backfill ticks missed while down or paused (capped). Off = only the next run."
			bind:checked={catchup}
		/>

		<div class="flex flex-col gap-1">
			<TextInput
				label="Max active runs"
				bind:value={maxActive}
				placeholder="unlimited"
				inputmode="numeric"
			/>
			<span class="text-[11px] text-text-tertiary">
				How many of this schedule's runs may be in-flight at once. 1 = wait for the previous run.
				Empty = unlimited.
			</span>
		</div>
	</div>

	{#snippet actions()}
		<Button variant="secondary" onclick={onClose} disabled={saving}>Cancel</Button>
		<Button variant="primary" onclick={save} loading={saving}>
			{isEdit ? 'Save' : 'Create'}
		</Button>
	{/snippet}
</Modal>
