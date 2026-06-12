/**
 * Advanced run options applied to the next preview/script run.
 *
 * `undefined` (or `null` for teamOwner) means "omit from the request payload"
 * so the backend uses its own default.
 */
export interface RunOptions {
	/** Parsed JSON value forwarded as the `args` field on createRun. */
	args?: unknown;
	/** Routing tag (worker scheduling label). */
	tag?: string;
	/** Max execution time in seconds (1 - 86400 recommended). */
	timeout?: number;
	/** Higher = picked first. 0 - 32767 (i16 max). */
	priority?: number;
	/** CPU cores (float, 0.1 - 32 recommended). */
	cpus?: number;
	/** Memory limit in MB (1 - 65536 recommended). */
	memoryMb?: number;
	/** Disk limit in MB (1 - 1048576 recommended). */
	diskMb?: number;
	/** Team to charge quotas against. null = no team. */
	teamOwner?: string | null;
}

/**
 * Soft validation hints — these are recommendations, not hard limits.
 * The backend may enforce stricter team quotas in the future.
 */
export const SOFT_LIMITS = {
	timeout: { min: 1, max: 86_400, default: 3600 },
	priority: { min: 0, max: 32_767, default: 0 },
	cpus: { min: 0.1, max: 32, default: 1.0 },
	memoryMb: { min: 1, max: 65_536, default: 512 },
	diskMb: { min: 1, max: 1_048_576, default: 1024 }
} as const;
