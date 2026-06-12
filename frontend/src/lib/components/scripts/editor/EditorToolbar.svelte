<script lang="ts">
	import { ChevronDown } from '@lucide/svelte';
	import type { ScriptLang } from '$lib/types';
	import Badge from '$lib/components/common/Badge.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import {
		DEFAULT_PYTHON_RUNTIME,
		LANGUAGE_OPTIONS,
		PYTHON_RUNTIME_OPTIONS
	} from './languages';
	import { LABELS } from './labels';
	import type { EditorDiagnostic, EditorStatus } from './types';

	interface Props {
		language?: ScriptLang;
		runtime?: string;
		requirementsOpen?: boolean;
		showRequirements: boolean;
		showRuntime: boolean;
		requirementsCount: number;
		disabled: boolean;
		readonly: boolean;
		status: EditorStatus;
		diagnostics: EditorDiagnostic[];
		onRun?: () => void | Promise<void>;
		onValidate?: () => void | Promise<void>;
		onFormat?: () => void | Promise<void>;
	}

	let {
		language = $bindable<ScriptLang>('python3'),
		runtime = $bindable(DEFAULT_PYTHON_RUNTIME),
		requirementsOpen = $bindable(false),
		showRequirements,
		showRuntime,
		requirementsCount,
		disabled,
		readonly,
		status,
		diagnostics,
		onRun,
		onValidate,
		onFormat
	}: Props = $props();

	const errorCount = $derived(diagnostics.filter((d) => d.severity === 'error').length);
	const warningCount = $derived(diagnostics.filter((d) => d.severity === 'warning').length);
	// Readonly viewer (e.g. run Code tab): show the language as a static label so
	// it's still informative but can't be changed.
	const languageLabel = $derived(
		LANGUAGE_OPTIONS.find((o) => o.value === language)?.label ?? language
	);
</script>

<div class="flex flex-wrap items-center gap-2 border-b border-border bg-surface-alt px-2 py-1.5">
	{#if readonly}
		<span class="shrink-0 px-1 text-xs font-medium text-text-secondary">{languageLabel}</span>
	{:else}
		<div class="w-28 shrink-0">
			<Select
				ariaLabel="Script language"
				options={LANGUAGE_OPTIONS}
				bind:value={language}
				compact
				{disabled}
			/>
		</div>
	{/if}

	{#if showRuntime}
		<div class="w-20 shrink-0">
			<Select
				ariaLabel="Python runtime version"
				options={PYTHON_RUNTIME_OPTIONS}
				bind:value={runtime}
				compact
				{disabled}
			/>
		</div>
	{/if}

	{#if showRequirements}
		<Button
			size="sm"
			variant="ghost"
			onclick={() => (requirementsOpen = !requirementsOpen)}
			disabled={disabled || readonly}
			aria-expanded={requirementsOpen}
		>
			Requirements
			{#if requirementsCount > 0}
				<Badge variant="ghost">{requirementsCount}</Badge>
			{/if}
			<ChevronDown size={12} class={requirementsOpen ? 'rotate-180 transition' : 'transition'} />
		</Button>
	{/if}

	<div class="flex-1"></div>

	{#if onValidate}
		<Button
			size="sm"
			variant="ghost"
			onclick={() => void onValidate?.()}
			disabled={disabled || status === 'validating'}
			loading={status === 'validating'}
		>
			{LABELS.toolbar.validate}
		</Button>
	{/if}

	{#if onFormat}
		<Button
			size="sm"
			variant="ghost"
			onclick={() => void onFormat?.()}
			disabled={disabled || readonly || status === 'formatting'}
			loading={status === 'formatting'}
		>
			{LABELS.toolbar.format}
		</Button>
	{/if}

	{#if onRun}
		<Button
			size="sm"
			variant="primary"
			onclick={() => void onRun?.()}
			disabled={disabled || readonly || status === 'running'}
			loading={status === 'running'}
		>
			{LABELS.toolbar.run}
		</Button>
	{/if}

	{#if status !== 'idle'}
		<Badge variant="info">{LABELS.status[status]}</Badge>
	{:else if diagnostics.length > 0}
		<Badge variant={errorCount > 0 ? 'error' : warningCount > 0 ? 'warning' : 'info'}>
			{errorCount > 0 ? `${errorCount} error${errorCount > 1 ? 's' : ''}` : ''}
			{errorCount > 0 && warningCount > 0 ? ', ' : ''}
			{warningCount > 0 ? `${warningCount} warning${warningCount > 1 ? 's' : ''}` : ''}
			{errorCount === 0 && warningCount === 0
				? `${diagnostics.length} issue${diagnostics.length > 1 ? 's' : ''}`
				: ''}
		</Badge>
	{/if}
</div>
