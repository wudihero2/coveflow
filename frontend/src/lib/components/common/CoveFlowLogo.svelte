<script lang="ts">
	interface Props {
		height?: number;
	}

	let { height = 36 }: Props = $props();

	// Crop the source SVG viewbox down to the actual logo artwork.
	const vbX = 58;
	const vbY = 26;
	const vbW = 82;
	const vbH = 118;

	// Deterministic unique ID avoids gradient clashes when multiple instances render.
	const uid = $props.id();
</script>

<!-- Decorative SVG logo; surrounding text provides the accessible brand name. -->
<svg
	xmlns="http://www.w3.org/2000/svg"
	viewBox="{vbX} {vbY} {vbW} {vbH}"
	width={height * (vbW / vbH)}
	{height}
	class="shrink-0"
	aria-hidden="true"
>
	<defs>
		<linearGradient id="{uid}-cove" x1="0%" y1="0%" x2="100%" y2="100%">
			<stop offset="0%" stop-color="#1E40AF" />
			<stop offset="40%" stop-color="#2563EB" />
			<stop offset="100%" stop-color="#1D4ED8" />
		</linearGradient>
		<linearGradient id="{uid}-wave" x1="0%" y1="0%" x2="100%" y2="0%">
			<stop offset="0%" stop-color="#0284C7" />
			<stop offset="50%" stop-color="#0EA5E9" />
			<stop offset="100%" stop-color="#14B8A6" />
		</linearGradient>
	</defs>

	<!-- C-shape cove -->
	<path
		d="M 120,32
		   A 53,53 0 0,0 67,85
		   A 53,53 0 0,0 120,138
		   L 127,138
		   A 41,41 0 0,1 86,85
		   A 41,41 0 0,1 127,32 Z"
		fill="url(#{uid}-cove)"
	/>

	<!-- Top wave + dot -->
	<path d="M 79,62 C 95,54 105,70 127,58" fill="none" stroke="url(#{uid}-wave)" stroke-width="9" stroke-linecap="round" />
	<circle cx="127" cy="58" r="5" fill="#FFF" stroke="#0EA5E9" stroke-width="3" />

	<!-- Middle wave + dot -->
	<path d="M 74,85 C 91,77 103,93 131,82" fill="none" stroke="url(#{uid}-wave)" stroke-width="10" stroke-linecap="round" />
	<circle cx="131" cy="82" r="5" fill="#FFF" stroke="#0EA5E9" stroke-width="3" />

	<!-- Bottom wave + dot -->
	<path d="M 79,108 C 95,100 105,116 127,104" fill="none" stroke="url(#{uid}-wave)" stroke-width="9" stroke-linecap="round" />
	<circle cx="127" cy="104" r="5" fill="#FFF" stroke="#14B8A6" stroke-width="3" />
</svg>
