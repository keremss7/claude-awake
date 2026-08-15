/// <reference types="vite/client" />

// Pulls in Vite's ambient module declarations, including the one for `*.css`.
// Without it, TypeScript 7 rejects the side-effect import in main.tsx (TS2882).
