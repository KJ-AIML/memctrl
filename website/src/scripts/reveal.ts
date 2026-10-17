/**
 * Scroll reveals for the home page: each `[data-reveal]` element gets `is-in` when it first
 * scrolls into view, and CSS staggers what's inside it (the brief's lines, the chart's squares).
 * Without JavaScript nothing is hidden; with reduced motion everything shows at once.
 */
export function mountReveals() {
  const items = document.querySelectorAll<HTMLElement>('[data-reveal]');
  if (!items.length) return;
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (reduce || !('IntersectionObserver' in window)) {
    items.forEach((item) => item.classList.add('is-in'));
    return;
  }
  document.documentElement.classList.add('mc-js');
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add('is-in');
        observer.unobserve(entry.target);
      }
    },
    { threshold: 0.18, rootMargin: '0px 0px -8% 0px' },
  );
  items.forEach((item) => observer.observe(item));
}
