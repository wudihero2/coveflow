# Frontend

SvelteKit + Svelte 5 + Tailwind CSS v4 + DaisyUI 5

## Prerequisites

- Node.js >= 20
- npm >= 10

## Commands

```bash
# Install dependencies
npm install

# Start dev server (localhost:5173, proxies /api to localhost:8000)
npm run dev

# Production build (outputs to build/)
npm run build

# Preview production build locally
npm run preview

# Type check (runs svelte-kit sync + svelte-check)
npm run check

# Regenerate .svelte-kit types (run after changing routes or config)
npx svelte-kit sync

# TypeScript compiler check (faster, no Svelte-specific checks)
npx tsc --noEmit
```

## Project Structure

```
src/
├── app.html          # HTML shell
├── app.css           # Tailwind + DaisyUI + theme tokens
├── lib/              # Shared code ($lib alias)
│   ├── types.ts
│   ├── toast.ts
│   ├── stores/
│   ├── services/
│   └── components/
└── routes/           # File-based routing
    ├── +layout.svelte
    ├── user/         # Auth pages (login, signup)
    └── (root)/       # Authenticated pages
```
