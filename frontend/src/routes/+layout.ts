// Disable SSR for the entire app.
//
// Our auth model stores the JWT access token in a module-level variable
// (memory only, never localStorage).  This only works in the browser —
// there's no server-side session to populate it from.
//
// adapter-static + fallback: 'index.html' already makes the build output
// a pure SPA, but without `ssr = false` SvelteKit still generates an SSR
// bundle and attempts server-side rendering during `vite build`.  Any
// component that imports the auth store at module level would break.
//
// Setting `ssr = false` here makes the intent explicit and prevents
// accidental SSR of auth-dependent code.
export const ssr = false;

// Prerender the SPA shell (index.html fallback).
export const prerender = false;
