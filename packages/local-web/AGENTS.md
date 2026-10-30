## New Design System Styling Guidelines

### CSS Variables & Tailwind Config

The new design uses custom CSS variables defined in `../web-core/src/app/styles/new/index.css` and configured in `tailwind.new.config.js`. All styles are scoped to the `.new-design` class.

### Colors

**Text colors** (use these instead of `text-gray-*`):
- `text-high` - Primary text, highest contrast
- `text-normal` - Standard text
- `text-low` - Muted/secondary text, placeholders
- `text-lowest` - Faint text for disabled or decorative hints only

**Background colors**:
- `bg-primary` - Main background
- `bg-secondary` - Slightly darker, used for inputs, cards, sidebars
- `bg-panel` - Chips, hover fills, subtle raised surfaces
- `bg-selected` - Selected rows/items (pale blue)
- `bg-rail` - App rail / top bar

**Accent colors** (palette lives in `--c-*` variables in `index.css`; light and dark):
- `brand` - Action blue for buttons, links and focus (`#0F67CB` light, `#4EA8FF` dark); `text-on-brand` for text on it
- `brand-accent` - Cisco Blue `#00BCEB` for indicators, progress and active markers; never small text on light backgrounds
- `kpi` - Numbers/KPIs
- `error`, `success`, `warning` - Status only (crit, ok, warn text tones)

### Typography

**Font families** (local system stacks, no web fonts):
- `font-sans` - Default sans-serif (`font-ibm-plex-sans` is a legacy alias)
- `font-mono` - Monospace/code (`font-ibm-plex-mono` is a legacy alias)

Numbers render with tabular numerals by default. Use sentence case for titles.

**Font sizes**:
- `text-xs` - 12px
- `text-sm` - 14px
- `text-base` - 16px (default)
- `text-lg` - 18px
- `text-xl` - 20px

### Spacing

Custom spacing tokens:
- `p-half` / `m-half` - 6px
- `p-base` / `m-base` - 12px
- `p-double` / `m-double` - 24px

### Border Radius

Uses a small radius by default (`--radius: 0.25rem`):
- `rounded` - Default small radius
- `rounded-sm`, `rounded-md`, `rounded-lg` - Progressively larger

### Focus States

Focus rings use `ring-brand` (action blue) and are inset by default.

### Shadows and motion

`shadow-sm`/`shadow`/`shadow-md` are a hairline card shadow in light mode and none in dark mode; `shadow-lg`/`shadow-xl` are for overlays. Keep hovers subtle (`hover:bg-panel`). Animations are disabled under `prefers-reduced-motion`.

### Example Component Styling

```tsx
// Input field
className="px-base bg-secondary rounded border text-base text-normal placeholder:text-low focus:outline-none focus:ring-1 focus:ring-brand"

// Button (icon)
className="flex items-center justify-center bg-secondary rounded border text-low hover:text-normal"

// Sidebar container
className="w-64 bg-secondary shrink-0 p-base"
```

### Architecture Rules

- **View components** (in `views/`) should be stateless - receive all data via props
- **Container components** (in `containers/`) manage state and pass to views
- **UI components** (in `ui-new/`) are reusable primitives
- File names in `ui-new/` must be **PascalCase** (e.g., `Field.tsx`, `Label.tsx`)
