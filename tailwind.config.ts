import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        canvas: "hsl(var(--canvas))",
        panel: "hsl(var(--panel))",
        ink: "hsl(var(--ink))",
        muted: "hsl(var(--muted))",
        accent: "hsl(var(--accent))",
        "accent-foreground": "hsl(var(--accent-foreground))",
        border: "hsl(var(--border))",
      },
      boxShadow: {
        soft: "0 18px 50px hsl(222 47% 11% / 0.08)",
      },
    },
  },
  plugins: [],
} satisfies Config;
