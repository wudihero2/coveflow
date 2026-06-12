// Client-side mirror of the backend's `AuthedUser::can_write` (auth.rs) for the
// three system roots, so the UI can hide actions the backend would 403. The
// backend remains the source of truth — this is UX gating only.
import type { UserInfo } from '$lib/types';

/** Can `me` write to `path`? Mirrors auth.rs can_write (3-roots model). */
export function canWrite(path: string, me: UserInfo | null | undefined): boolean {
	if (!me) return false;
	// Mirror auth.rs: the bypass is the *workspace* role Admin, NOT instance admin
	// (get_me returns them as distinct fields; can_write only looks at role).
	if (me.role === 'admin') return true;

	const usersRest = stripPrefix(path, 'users/');
	if (usersRest !== null) return rootSegment(usersRest) === me.email;

	const teamsRest = stripPrefix(path, 'teams/');
	if (teamsRest !== null) return me.writable_teams.includes(rootSegment(teamsRest));

	if (path.startsWith('workspace/')) return me.role !== 'viewer';

	return false;
}

function stripPrefix(path: string, prefix: string): string | null {
	return path.startsWith(prefix) ? path.slice(prefix.length) : null;
}

function rootSegment(rest: string): string {
	const slash = rest.indexOf('/');
	return slash === -1 ? rest : rest.slice(0, slash);
}
