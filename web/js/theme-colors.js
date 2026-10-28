// theme-colors.js
// Bridges the CSS custom-property theme (web/css/theme-*.css) into the Canvas 2D
// renderers. The canvas API cannot resolve `var()` or `color-mix()` expressions,
// so every hardcoded color inside the drawing modules is replaced with a lookup
// of a plain-hex theme token performed here.
//
// Values are cached after the first read and invalidated whenever the theme
// changes (the "cnc-theme-changed" event dispatched by ui-settings.js), which
// keeps per-frame drawing cost negligible.

/** Cache of token name -> resolved hex string, cleared on theme change. */
let cache = new Map();

// Invalidate the cache as soon as this module is imported, so any renderer
// asking for a color after a theme switch always gets fresh values.
window.addEventListener("cnc-theme-changed", () => {
  cache = new Map();
});

/**
 * Reads a theme token (CSS custom property) as a resolved `#rrggbb` string.
 *
 * Falls back to the provided default when the property is missing or empty
 * (e.g. a token typo or a stylesheet that failed to load), so renderers never
 * receive an invalid canvas color.
 *
 * @param {string} name - Custom property name, e.g. "--accent".
 * @param {string} [fallback="#000000"] - Value used when the token is absent.
 * @returns {string} Hex color like "#287eaa".
 */
export function themeToken(name, fallback = "#000000") {
  if (cache.has(name)) return cache.get(name);
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  const value = /^#[0-9a-fA-F]{3,8}$/.test(raw) ? raw : fallback;
  cache.set(name, value);
  return value;
}

/**
 * Parses a `#rgb` / `#rrggbb` color into 0..255 channel values.
 *
 * @param {string} hex - Token color in short or long hex form.
 * @returns {[number, number, number]} Red, green and blue channels.
 */
function hexToRgb(hex) {
  const shorthand = /^#[0-9a-fA-F]{3}$/.test(hex);
  const body = hex.slice(1);
  const expanded = shorthand ? body.split("").map((c) => c + c).join("") : body;
  const num = Number.parseInt(expanded.slice(0, 6), 16);
  return [(num >> 16) & 255, (num >> 8) & 255, num & 255];
}

/**
 * Returns a theme token as an `rgba()` string with a fixed alpha.
 *
 * Canvas fills/strokes often need partial transparency (sector washes, grid
 * lines). Because the tokens are stored as opaque hex values, the alpha is
 * applied here at read time instead of baking rgba literals into the JS.
 *
 * @param {string} name - Custom property name, e.g. "--dial-hover-fill".
 * @param {number} alpha - Opacity 0..1.
 * @param {string} [fallback="#000000"] - Hex used when the token is missing.
 * @returns {string} CSS `rgba(r, g, b, a)` color for canvas usage.
 */
export function themeAlpha(name, alpha, fallback = "#000000") {
  const [r, g, b] = hexToRgb(themeToken(name, fallback));
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
