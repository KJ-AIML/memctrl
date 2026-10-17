/**
 * The home page hero's pixel field: periwinkle squares piled on the left and dots on the right,
 * meeting along the bottom.
 *
 * On load the piles pour in like an avalanche. Pixels tumble in over each side edge from the top
 * of the mountain, fall under gravity onto the pile built so far, and slide down its surface,
 * faster where it's steep and slowed by friction, until they reach their place. The bottom layer
 * fills first, so later pixels ride down over earlier ones. Afterwards the pointer pushes nearby
 * pixels away and they spring back, and a click knocks them loose so they fall back into place.
 * With reduced motion, the field is drawn settled and stays still.
 */

const PALETTE: [string, number][] = [
  ['#8b93ff', 0.46],
  ['#9aa1ff', 0.16],
  ['#a9afff', 0.14],
  ['#c9ccff', 0.14],
  ['#e9eaff', 0.1],
];

const GRAVITY = 1700; // px/s²
const SLOPE_PULL = 1100; // px/s² along the surface per cell of drop ahead
const FRICTION = 1.4; // per second, while sliding
const MIN_SLIDE = 55; // px/s: sand keeps creeping toward its place
const MAX_SLIDE = 520;
const POUR_SECONDS = 5; // how long each side pours
const LAND_SPRING = 150; // pulls a pixel into its slot once it arrives
const LAND_DAMPING = 15;
const SPRING = 70; // holds settled pixels in place
const DAMPING = 10;
const BOUNCE = 0.3;
const PUSH_RADIUS = 90;
const PUSH_FORCE = 9000;

type Pixel = {
  tx: number;
  ty: number;
  col: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
  dir: 1 | -1; // which way it pours: +1 from the left edge, -1 from the right
  /** wait: not poured yet; pour: falling and sliding; land: easing into its slot; settled: in
   *  place; loose: knocked loose by a click. */
  state: 'wait' | 'pour' | 'land' | 'settled' | 'loose';
  release: number; // seconds after load when it pours in
  shape: 'square' | 'dot';
  color: string;
};

function rng(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function pick(r: number) {
  let sum = 0;
  for (const [color, weight] of PALETTE) {
    sum += weight;
    if (r < sum) return color;
  }
  return PALETTE[0][0];
}

type Field = { cell: number; cols: number; rows: number; pixels: Pixel[]; edgeTop: [number, number] };

/** Lays the two mountains out for a hero of this size, with each pixel's pour order. */
function layout(width: number, height: number): Field {
  const cell = width < 640 ? 14 : width < 1100 ? 18 : 22;
  const cols = Math.ceil(width / cell);
  const rows = Math.floor(height / cell);
  const mid = Math.floor(cols / 2);
  // The piles reach about half the hero's height at the edges and taper to two rows in the middle,
  // below the hero text.
  const peak = Math.max(4, Math.floor(rows * (height < 720 ? 0.4 : 0.5)));
  const random = rng(7);
  const pixels: Pixel[] = [];
  const tallest: [number, number] = [0, 0];
  for (let c = 0; c < cols; c++) {
    const left = c < mid;
    const t = left ? c / mid : (c - mid) / Math.max(1, cols - mid - 1);
    const shape = left ? 1 - Math.pow(t, 0.85) : Math.pow(t, 1.25) * 0.9;
    const noise = (random() - 0.5) * 2.4;
    const tall = Math.max(1, Math.min(rows, Math.round(2 + (peak - 2) * shape + noise)));
    tallest[left ? 0 : 1] = Math.max(tallest[left ? 0 : 1], tall);
    const add = (row: number) => {
      pixels.push({
        tx: c * cell,
        ty: row * cell,
        col: c,
        x: 0,
        y: 0,
        vx: 0,
        vy: 0,
        dir: left ? 1 : -1,
        state: 'wait',
        release: 0,
        shape: left ? 'square' : 'dot',
        color: pick(random()),
      });
    };
    for (let row = rows - tall; row < rows; row++) {
      if (random() < 0.035) continue; // a few dark gaps
      add(row);
    }
    if (random() < 0.28 && tall < rows - 2) add(rows - tall - 1 - Math.floor(random() * 3));
  }
  // Pour order on each side: the bottom layer first, and in each layer the pixels bound farthest
  // from the edge first, so sand runs all the way down before the pile grows back up the slope.
  for (const dir of [1, -1] as const) {
    const side = pixels.filter((p) => p.dir === dir);
    const fromEdge = (p: Pixel) => (dir === 1 ? p.col : cols - 1 - p.col);
    side.sort((a, b) => b.ty - a.ty || fromEdge(b) - fromEdge(a));
    side.forEach((p, i) => {
      p.release = (i / side.length) * POUR_SECONDS + random() * 0.12;
    });
  }
  return { cell, cols, rows, pixels, edgeTop: [(rows - tallest[0]) * cell, (rows - tallest[1]) * cell] };
}

export function mountPixelField(hero: HTMLElement, canvas: HTMLCanvasElement) {
  const context = canvas.getContext('2d');
  if (!context) return;
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  let field: Field = { cell: 22, cols: 0, rows: 0, pixels: [], edgeTop: [0, 0] };
  /** The top of the settled pile in each column: where a sliding pixel's bottom rests. */
  let surface: number[] = [];
  let width = 0;
  let height = 0;
  let elapsed = 0;
  let last = 0;
  let frame = 0;
  let visible = true;
  const pointer = { x: -1e9, y: -1e9 };

  function resize(fresh: boolean) {
    const box = hero.getBoundingClientRect();
    width = Math.round(box.width);
    height = Math.round(box.height);
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    context!.setTransform(ratio, 0, 0, ratio, 0, 0);
    field = layout(width, height);
    surface = new Array(field.cols + 2).fill(field.rows * field.cell);
    elapsed = 0;
    for (const p of field.pixels) {
      if (fresh && !reduce) continue;
      p.state = 'settled';
      p.x = p.tx;
      p.y = p.ty;
    }
  }

  /** Where a pixel's top rests at x: on the higher of the two columns it overlaps. */
  function restY(x: number) {
    const { cell, cols } = field;
    const c0 = Math.max(0, Math.min(cols - 1, Math.floor(x / cell)));
    const c1 = Math.max(0, Math.min(cols - 1, Math.ceil(x / cell)));
    return Math.min(surface[c0], surface[c1]) - cell;
  }

  function draw() {
    const { cell } = field;
    context!.clearRect(0, 0, width, height);
    const gap = 1.4;
    for (const p of field.pixels) {
      if (p.state === 'wait') continue;
      context!.fillStyle = p.color;
      if (p.shape === 'square') {
        context!.fillRect(p.x, p.y, cell, cell);
      } else {
        context!.beginPath();
        context!.arc(p.x + cell / 2, p.y + cell / 2, cell / 2 - gap, 0, Math.PI * 2);
        context!.fill();
      }
    }
  }

  /** Advances the simulation; returns whether anything still moves. */
  function step(dt: number) {
    const { cell, cols } = field;
    let moving = false;
    for (const p of field.pixels) {
      switch (p.state) {
        case 'wait': {
          moving = true;
          if (elapsed < p.release) break;
          // Tumble in over the edge, from the top of that side's mountain.
          const top = field.edgeTop[p.dir === 1 ? 0 : 1];
          p.x = p.dir === 1 ? -cell * 1.1 : width + cell * 0.1;
          p.y = top - cell * (1 + Math.random() * 2.5);
          p.vx = p.dir * (90 + Math.random() * 140);
          p.vy = -Math.random() * 80;
          p.state = 'pour';
          break;
        }
        case 'pour': {
          moving = true;
          p.vy += GRAVITY * dt;
          let y = p.y + p.vy * dt;
          const rest = restY(p.x);
          const onSurface = y >= rest;
          if (onSurface) {
            // Sliding: pulled downhill by the drop ahead, slowed by friction.
            y = rest;
            p.vy = 0;
            const col = Math.max(0, Math.min(cols - 1, Math.round(p.x / cell)));
            const ahead = Math.max(0, Math.min(cols - 1, col + p.dir));
            const drop = Math.max(0, Math.min(1.5, (surface[ahead] - surface[col]) / cell));
            p.vx += p.dir * SLOPE_PULL * drop * dt;
            p.vx -= p.vx * FRICTION * dt;
            if (p.vx * p.dir < MIN_SLIDE) p.vx = p.dir * MIN_SLIDE;
          }
          if (Math.abs(p.vx) > MAX_SLIDE) p.vx = Math.sign(p.vx) * MAX_SLIDE;
          p.x += p.vx * dt;
          p.y = y;
          // It stops where it belongs, and the pile there grows by one.
          if ((p.dir === 1 && p.x >= p.tx) || (p.dir === -1 && p.x <= p.tx)) {
            p.state = 'land';
            surface[p.col] = Math.min(surface[p.col], p.ty);
          }
          break;
        }
        case 'land': {
          moving = true;
          p.vx += (LAND_SPRING * (p.tx - p.x) - LAND_DAMPING * p.vx) * dt;
          p.vy += (LAND_SPRING * (p.ty - p.y) - LAND_DAMPING * p.vy) * dt;
          p.x += p.vx * dt;
          p.y += p.vy * dt;
          if (Math.abs(p.x - p.tx) + Math.abs(p.y - p.ty) < 0.6 && Math.abs(p.vx) + Math.abs(p.vy) < 6) {
            p.state = 'settled';
            p.x = p.tx;
            p.y = p.ty;
            p.vx = 0;
            p.vy = 0;
          }
          break;
        }
        case 'loose': {
          moving = true;
          p.vy += GRAVITY * 1.5 * dt;
          p.vx += (SPRING * (p.tx - p.x) - DAMPING * p.vx) * dt;
          p.x += p.vx * dt;
          p.y += p.vy * dt;
          if (p.y >= p.ty) {
            p.y = p.ty;
            if (Math.abs(p.vy) > 120) {
              p.vy = -p.vy * BOUNCE;
            } else {
              p.vy = 0;
              p.state = 'settled';
            }
          }
          break;
        }
        case 'settled': {
          // A spring holds each pixel to its place, and the pointer pushes it away.
          let ax = SPRING * (p.tx - p.x) - DAMPING * p.vx;
          let ay = SPRING * (p.ty - p.y) - DAMPING * p.vy;
          const dx = p.x + cell / 2 - pointer.x;
          const dy = p.y + cell / 2 - pointer.y;
          const distance = Math.hypot(dx, dy);
          if (distance < PUSH_RADIUS && distance > 0.01) {
            const strength = (1 - distance / PUSH_RADIUS) * PUSH_FORCE;
            ax += (dx / distance) * strength;
            ay += (dy / distance) * strength;
          }
          p.vx += ax * dt;
          p.vy += ay * dt;
          p.x += p.vx * dt;
          p.y += p.vy * dt;
          if (Math.abs(p.vx) + Math.abs(p.vy) > 2 || Math.abs(p.x - p.tx) + Math.abs(p.y - p.ty) > 0.5) {
            moving = true;
          } else {
            p.x = p.tx;
            p.y = p.ty;
            p.vx = 0;
            p.vy = 0;
          }
          break;
        }
      }
    }
    return moving;
  }

  function tick(now: number) {
    frame = 0;
    const dt = Math.min(0.033, last ? (now - last) / 1000 : 0.016);
    last = now;
    elapsed += dt;
    // Two substeps keep fast pixels from skipping over thin parts of the pile.
    const first = step(dt / 2);
    const moving = step(dt / 2) || first;
    draw();
    if (visible && (moving || pointer.x > -1e8)) frame = requestAnimationFrame(tick);
    else last = 0;
  }

  function wake() {
    if (!reduce && visible && !frame) frame = requestAnimationFrame(tick);
  }

  resize(true);
  draw();
  if (reduce) return;
  wake();

  hero.addEventListener('pointermove', (event) => {
    const box = hero.getBoundingClientRect();
    pointer.x = event.clientX - box.left;
    pointer.y = event.clientY - box.top;
    wake();
  });
  hero.addEventListener('pointerleave', () => {
    pointer.x = pointer.y = -1e9;
    wake();
  });
  hero.addEventListener('pointerdown', (event) => {
    if ((event.target as Element).closest('a, button, code')) return;
    const box = hero.getBoundingClientRect();
    const x = event.clientX - box.left;
    const y = event.clientY - box.top;
    // Knock nearby settled pixels loose: they fly up and away, then fall back into place.
    for (const p of field.pixels) {
      if (p.state !== 'settled') continue;
      const dx = p.x + field.cell / 2 - x;
      const dy = p.y + field.cell / 2 - y;
      const distance = Math.hypot(dx, dy);
      if (distance < 220) {
        const strength = (1 - distance / 220) * 1100;
        p.vx += (dx / (distance || 1)) * strength;
        p.vy += -Math.abs(strength) * (0.6 + Math.random() * 0.6);
        p.state = 'loose';
      }
    }
    wake();
  });

  // Lay the field out again only when the hero's size really changes (the observer also fires
  // once when attached, which mustn't cancel the pour).
  let resizeTimer = 0;
  new ResizeObserver(() => {
    window.clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(() => {
      const box = hero.getBoundingClientRect();
      if (Math.round(box.width) === width && Math.round(box.height) === height) return;
      resize(false);
      draw();
    }, 120);
  }).observe(hero);
  new IntersectionObserver(([entry]) => {
    visible = entry.isIntersecting;
    wake();
  }).observe(hero);
}
