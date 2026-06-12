// Single source of truth for which /admin routes are gated on the instance-admin
// flag (account.is_admin) rather than the workspace role. The admin layout uses
// this to decide access; keeping the rule here means adding another
// instance-admin route is a one-line change instead of touching every guard.

/** Prefixes under /admin that require the instance-admin flag, not just a
 *  workspace-admin role. Each entry matches the exact path or any sub-path. */
const INSTANCE_ADMIN_PREFIXES = ['/admin/cluster'] as const;

/** True when `pathname` is an instance-admin-only admin route. */
export function isInstanceAdminPath(pathname: string): boolean {
	return INSTANCE_ADMIN_PREFIXES.some(
		(prefix) => pathname === prefix || pathname.startsWith(`${prefix}/`)
	);
}
