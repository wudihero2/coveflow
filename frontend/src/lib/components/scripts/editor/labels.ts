export const LABELS = {
	toolbar: {
		run: 'Run',
		validate: 'Validate',
		format: 'Format'
	},
	status: {
		idle: 'Ready',
		loading: 'Loading editor...',
		validating: 'Validating...',
		formatting: 'Formatting...',
		running: 'Running...'
	},
	requirements: {
		label: 'Requirements',
		placeholder: 'Comma-separated pip packages, e.g. requests==2.31, pandas'
	},
	diagnostics: {
		title: 'Issues'
	},
	fallback: {
		notice: 'Monaco editor failed to load. Using plain text editor as fallback.',
		label: 'Script Content (fallback)'
	}
} as const;
