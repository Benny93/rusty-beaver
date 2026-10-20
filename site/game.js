// Beaver Bomber: a small Bomberman-style game for the project page.
// The rules (level generation, blasts, walking) are plain functions; the browser part
// below them draws pixel-art sprites on a canvas. scripts/game_test.js checks the rules.

const W = 15, H = 11;              // board in tiles
const FLOOR = 0, WALL = 1, CRATE = 2;

// Deterministic random numbers, so a level can be replayed and tested.
function rng(seed) {
  let s = seed >>> 0 || 1;
  return () => ((s = (s * 1664525 + 1013904223) >>> 0) / 4294967296);
}

const isSpawn = (x, y) => x + y <= 4; // keep the corner around the start free

function makeLevel(level, rand) {
  const tiles = [];
  for (let y = 0; y < H; y++) {
    const row = [];
    for (let x = 0; x < W; x++) {
      const border = x === 0 || y === 0 || x === W - 1 || y === H - 1;
      const pillar = x % 2 === 0 && y % 2 === 0;
      row.push(border || pillar ? WALL : !isSpawn(x, y) && rand() < 0.42 ? CRATE : FLOOR);
    }
    tiles.push(row);
  }
  // Power-ups hide under some crates.
  const items = {};
  for (let y = 0; y < H; y++)
    for (let x = 0; x < W; x++)
      if (tiles[y][x] === CRATE && rand() < 0.18) items[x + ',' + y] = rand() < 0.5 ? 'bomb' : 'range';
  // Enemies start on free floor, far from the player.
  const free = [];
  for (let y = 0; y < H; y++)
    for (let x = 0; x < W; x++)
      if (tiles[y][x] === FLOOR && x + y >= 8) free.push([x, y]);
  const enemies = [];
  for (let n = Math.min(2 + level, 9); n > 0 && free.length; n--) {
    const [x, y] = free.splice(Math.floor(rand() * free.length), 1)[0];
    enemies.push({ x, y, dir: [1, 0] });
  }
  return { tiles, items, enemies };
}

// Cells a bomb at (x, y) reaches: a cross that stops at walls, and at the first crate,
// which burns with it.
function blast(tiles, x, y, range) {
  const cells = [[x, y]];
  for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
    for (let i = 1; i <= range; i++) {
      const cx = x + dx * i, cy = y + dy * i;
      const t = tiles[cy][cx];
      if (t === WALL) break;
      cells.push([cx, cy]);
      if (t === CRATE) break;
    }
  }
  return cells;
}

// Explode `first` and every bomb its flames reach. Returns the burning cells and the
// bombs that went off.
function explode(tiles, bombs, first) {
  const gone = new Set([first]), queue = [first], cells = [];
  while (queue.length) {
    const b = queue.shift();
    for (const c of blast(tiles, b.x, b.y, b.range)) {
      cells.push(c);
      for (const o of bombs)
        if (!gone.has(o) && o.x === c[0] && o.y === c[1]) {
          gone.add(o);
          queue.push(o);
        }
    }
  }
  return { cells, bombs: [...gone] };
}

const walkable = (tiles, bombs, x, y) =>
  tiles[y]?.[x] === FLOOR && !bombs.some(b => b.x === x && b.y === y);

// ---- sprites --------------------------------------------------------------------------

// Pixel-art sprites: one character per pixel, '.' is transparent.
const PAL = {
  k: '#1b1216', g: '#4f7a32', G: '#79a157', b: '#8c5a2b', B: '#b07a45', d: '#5e3a1c',
  w: '#f4f1e4', l: '#d9b38c', m: '#4a3020', s: '#f2c46a', r: '#e0702a', R: '#c2402e',
  p: '#7b4fb0', P: '#a77bd8', n: '#3a2a50', x: '#3b3b46', X: '#5b5b6a', o: '#22222a',
};
const ART = {
  beaver: [
    '.....kkkkkk.....',
    '....kGGGGGGk....',
    '...kGGgGGgGGk...',
    '..kkkkkkkkkkkk..',
    '...kBBBBBBBBk...',
    '..kBBkkBBkkBBk..',
    '..kBBkwBBkwBBk..',
    '..kBBBBddBBBBk..',
    '...kBBkwwkBBk...',
    '...kbbkwwkbbk...',
    '..kbbblllbbbbk..',
    '.kbbbllllllbbbk.',
    'kmkbbllllllbbk..',
    'kmmkbbllllbbk...',
    '.kmkbbkkkkbbk...',
    '..k.kkk..kkk....',
  ],
  bug: [
    '................',
    '....k......k....',
    '.....k....k.....',
    '......kkkk......',
    '....kkPPPPkk....',
    '...kPPPPPPPPk...',
    '..kPPwkPPwkPPk..',
    '..kPPkkPPkkPPk..',
    'k.kpppppppppPk.k',
    '.kkpppnnnnpppkk.',
    '..kppppppppppk..',
    'k.kpppnnnnpppk.k',
    '.kkppppppppppkk.',
    '...kppppppppk...',
    '..k.kkkkkkkk.k..',
    '................',
  ],
  bomb: [
    '..........s.....',
    '.........s.s....',
    '..........k.....',
    '.........k......',
    '......kkkkk.....',
    '....kkoooookk...',
    '...koooooooook..',
    '..kooXXooooooook',
    '..kooXooooooooo.',
    '..koooooooooook.',
    '..koooooooooook.',
    '..koooooooooook.',
    '...koooooooook..',
    '....kkoooookk...',
    '......kkkkk.....',
    '................',
  ],
};

const Rules = { W, H, FLOOR, WALL, CRATE, rng, makeLevel, blast, explode, walkable, PAL, ART };
if (typeof module !== 'undefined') module.exports = Rules;

// ---- browser ---------------------------------------------------------------------------

if (typeof document !== 'undefined') (() => {
  const root = document.getElementById('beaver-bomber');
  if (!root) return;
  const canvas = root.querySelector('canvas'), g = canvas.getContext('2d');
  const overlay = root.querySelector('.overlay'), hud = root.querySelector('.hud');
  const T = 16;

  const sprites = {};
  const sprite = (name, draw) => {
    const c = document.createElement('canvas');
    c.width = c.height = T;
    draw(c.getContext('2d'));
    sprites[name] = c;
  };
  for (const [name, rows] of Object.entries(ART))
    sprite(name, s => rows.forEach((row, y) => [...row].forEach((ch, x) => {
      if (PAL[ch]) { s.fillStyle = PAL[ch]; s.fillRect(x, y, 1, 1); }
    })));
  const rect = (s, c, x, y, w, h) => { s.fillStyle = c; s.fillRect(x, y, w, h); };
  sprite('floor', s => { rect(s, '#c9a36b', 0, 0, T, T); rect(s, '#bf9760', 0, 15, T, 1); rect(s, '#bf9760', 15, 0, 1, T); rect(s, '#d4ae76', 3, 4, 2, 1); rect(s, '#d4ae76', 10, 10, 2, 1); });
  sprite('wall', s => {
    rect(s, '#3b3b46', 0, 0, T, T);
    for (const [y, off] of [[0, 0], [5, 4], [10, 0]]) for (let x = -off; x < T; x += 8) rect(s, '#5b5b6a', x + 1, y + 1, 6, 3);
    rect(s, '#22222a', 0, 15, T, 1);
  });
  sprite('crate', s => {
    rect(s, '#1b1216', 0, 0, T, T);
    for (const y of [1, 6, 11]) { rect(s, '#8c5a2b', 1, y, 14, 4); rect(s, '#b07a45', 1, y, 14, 1); rect(s, '#d9b38c', 12, y + 1, 2, 2); rect(s, '#5e3a1c', 13, y + 1, 1, 1); }
  });
  sprite('flame', s => { rect(s, '#e0702a', 1, 1, 14, 14); rect(s, '#f2c46a', 4, 4, 8, 8); rect(s, '#fff6d5', 6, 6, 4, 4); });
  const item = (name, color) => sprite(name, s => {
    rect(s, '#1b1216', 1, 1, 14, 14); rect(s, '#f2c46a', 2, 2, 12, 12);
    s.drawImage(sprites.bomb, 3, 3, 10, 10);
    if (color) { rect(s, color, 5, 7, 6, 3); rect(s, '#f2c46a', 6, 8, 4, 1); }
  });
  item('bomb-up');
  item('range-up', '#e0702a');

  // ---- state --------------------------------------------------------------------------
  const STEP = 140, FUSE = 2200, BURN = 450;
  let s, mode = 'title', held = [], last = 0, rand = rng(Date.now());
  const fresh = () => ({ level: 1, score: 0, lives: 3, maxBombs: 1, range: 2 });
  let run = fresh();

  function startLevel() {
    const lv = makeLevel(run.level, rand);
    s = {
      tiles: lv.tiles, items: lv.items, shown: {},
      enemies: lv.enemies.map(e => ({ ...e, px: e.x, py: e.y, t: 0, alive: true })),
      player: { x: 1, y: 1, px: 1, py: 1, t: 0 },
      bombs: [], flames: [], clock: 0,
    };
    // Enemies get quicker on later levels.
    s.enemyStep = Math.max(220, 520 - run.level * 35);
  }

  function show(text, sub) {
    overlay.innerHTML = `<strong>${text}</strong>${sub ? `<span>${sub}</span>` : ''}`;
    overlay.hidden = false;
  }

  function setMode(m) {
    mode = m;
    root.classList.toggle('playing', m === 'play');
    if (m === 'play') overlay.hidden = true;
    if (m === 'title') show('BEAVER BOMBER', 'Press Space or tap to start');
    if (m === 'pause') show('PAUSED', 'Space to continue');
    if (m === 'clear') show(`LEVEL ${run.level} CLEAR!`, 'Space for the next level');
    if (m === 'dead') show('OUCH!', run.lives > 0 ? 'Space to try again' : '');
    if (m === 'over') show('GAME OVER', `Score ${run.score} · Space to play again`);
    updateHud();
  }

  function updateHud() {
    hud.textContent = `LEVEL ${run.level}   SCORE ${run.score}   LIVES ${'♥'.repeat(Math.max(run.lives, 0))}   BOMBS ${run.maxBombs}   RANGE ${run.range}`;
  }

  function primary() {
    if (mode === 'title' || mode === 'over') { run = fresh(); startLevel(); setMode('play'); }
    else if (mode === 'pause') setMode('play');
    else if (mode === 'clear') { run.level++; startLevel(); setMode('play'); }
    else if (mode === 'dead') {
      if (run.lives > 0) { startLevel(); setMode('play'); } else setMode('over');
    } else if (mode === 'play') dropBomb();
  }

  function dropBomb() {
    const p = s.player;
    if (s.bombs.length >= run.maxBombs || s.bombs.some(b => b.x === p.x && b.y === p.y)) return;
    s.bombs.push({ x: p.x, y: p.y, range: run.range, t: FUSE });
  }

  function die() {
    run.lives--;
    setMode(run.lives > 0 ? 'dead' : 'over');
  }

  // ---- update ---------------------------------------------------------------------------
  const DIRS = { up: [0, -1], down: [0, 1], left: [-1, 0], right: [1, 0] };

  function update(dt) {
    s.clock += dt;
    const p = s.player;
    // Player: one tile per step while a direction is held. Its own fresh bomb is passable.
    p.t = Math.max(0, p.t - dt);
    if (p.t === 0) {
      p.px = p.x; p.py = p.y;
      const d = DIRS[held[held.length - 1]];
      if (d) {
        const nx = p.x + d[0], ny = p.y + d[1];
        if (walkable(s.tiles, s.bombs, nx, ny)) { p.x = nx; p.y = ny; p.t = STEP; }
      }
    }
    // Bombs tick; a bomb in flames goes off at once.
    for (const b of s.bombs) b.t -= dt;
    const due = s.bombs.find(b => b.t <= 0 || s.flames.some(f => f.x === b.x && f.y === b.y));
    if (due) {
      const { cells, bombs } = explode(s.tiles, s.bombs, due);
      s.bombs = s.bombs.filter(b => !bombs.includes(b));
      for (const [x, y] of cells) {
        s.flames.push({ x, y, t: BURN });
        if (s.tiles[y][x] === CRATE) {
          s.tiles[y][x] = FLOOR;
          run.score += 10;
          if (s.items[x + ',' + y]) s.shown[x + ',' + y] = s.items[x + ',' + y];
        } else delete s.shown[x + ',' + y]; // flames burn loose power-ups
      }
    }
    for (const f of s.flames) f.t -= dt;
    s.flames = s.flames.filter(f => f.t > 0);
    const burning = (x, y) => s.flames.some(f => f.x === x && f.y === y);
    // Enemies wander: keep going while they can, turn at random otherwise.
    for (const e of s.enemies.filter(e => e.alive)) {
      if (burning(e.x, e.y)) { e.alive = false; run.score += 100; continue; }
      e.t -= dt;
      if (e.t > 0) continue;
      e.px = e.x; e.py = e.y; e.t = s.enemyStep;
      const options = Object.values(DIRS).filter(([dx, dy]) => walkable(s.tiles, s.bombs, e.x + dx, e.y + dy));
      if (!options.length) continue;
      const ahead = options.find(([dx, dy]) => dx === e.dir[0] && dy === e.dir[1]);
      e.dir = ahead && rand() < 0.75 ? ahead : options[Math.floor(rand() * options.length)];
      e.x += e.dir[0]; e.y += e.dir[1];
    }
    // Power-ups.
    const key = p.x + ',' + p.y, got = s.shown[key];
    if (got) {
      if (got === 'bomb') run.maxBombs = Math.min(run.maxBombs + 1, 8);
      else run.range = Math.min(run.range + 1, 8);
      delete s.shown[key];
      delete s.items[key];
      run.score += 50;
      updateHud();
    }
    if (burning(p.x, p.y) || s.enemies.some(e => e.alive && e.x === p.x && e.y === p.y)) return die();
    if (s.enemies.every(e => !e.alive)) { run.score += 500; return setMode('clear'); }
    updateHud();
  }

  // ---- draw -----------------------------------------------------------------------------
  const lerp = (a, b, t) => a + (b - a) * t;

  function draw() {
    for (let y = 0; y < H; y++)
      for (let x = 0; x < W; x++)
        g.drawImage(sprites[['floor', 'wall', 'crate'][s.tiles[y][x]]], x * T, y * T);
    for (const [k, v] of Object.entries(s.shown)) {
      const [x, y] = k.split(',').map(Number);
      g.drawImage(sprites[v === 'bomb' ? 'bomb-up' : 'range-up'], x * T, y * T);
    }
    for (const b of s.bombs) {
      const pulse = Math.floor(s.clock / 150) % 2;
      g.drawImage(sprites.bomb, b.x * T + pulse * 0.5, b.y * T + pulse * 0.5, T - pulse, T - pulse);
    }
    for (const f of s.flames) {
      g.globalAlpha = Math.min(1, f.t / 150);
      g.drawImage(sprites.flame, f.x * T, f.y * T);
      g.globalAlpha = 1;
    }
    for (const e of s.enemies.filter(e => e.alive)) {
      const k = 1 - e.t / s.enemyStep;
      g.drawImage(sprites.bug, Math.round(lerp(e.px, e.x, k) * T), Math.round(lerp(e.py, e.y, k) * T));
    }
    const p = s.player, k = p.t ? 1 - p.t / STEP : 1;
    g.drawImage(sprites.beaver, Math.round(lerp(p.px, p.x, k) * T), Math.round(lerp(p.py, p.y, k) * T));
  }

  function frame(now) {
    const dt = Math.min(50, now - (last || now));
    last = now;
    if (mode === 'play') update(dt);
    if (s) draw();
    requestAnimationFrame(frame);
  }

  // ---- input ----------------------------------------------------------------------------
  const KEYS = { ArrowUp: 'up', KeyW: 'up', ArrowDown: 'down', KeyS: 'down', ArrowLeft: 'left', KeyA: 'left', ArrowRight: 'right', KeyD: 'right' };
  const press = d => { held = held.filter(h => h !== d); held.push(d); };
  const release = d => { held = held.filter(h => h !== d); };
  // Keys belong to the game only while it runs or has focus, so the page scrolls normally
  // otherwise; scrolling the game out of view pauses it (and so hands the keys back).
  const ours = () => mode === 'play' || root.contains(document.activeElement);

  window.addEventListener('keydown', e => {
    if (!ours()) return;
    if (KEYS[e.code]) { press(KEYS[e.code]); e.preventDefault(); }
    else if (e.code === 'Space' || e.code === 'Enter') { if (!e.repeat) primary(); e.preventDefault(); }
    else if ((e.code === 'KeyP' || e.code === 'Escape') && mode === 'play') setMode('pause');
  });
  window.addEventListener('keyup', e => { if (KEYS[e.code]) release(KEYS[e.code]); });
  window.addEventListener('blur', () => { held = []; if (mode === 'play') setMode('pause'); });
  document.addEventListener('visibilitychange', () => { if (document.hidden && mode === 'play') setMode('pause'); });
  new IntersectionObserver(([e]) => {
    if (!e.isIntersecting && mode === 'play') setMode('pause');
  }, { threshold: 0.4 }).observe(canvas);

  overlay.addEventListener('click', () => { root.focus(); primary(); });
  // Touch / mouse buttons.
  root.querySelectorAll('[data-dir]').forEach(btn => {
    const d = btn.dataset.dir;
    btn.addEventListener('pointerdown', e => { e.preventDefault(); press(d); });
    for (const ev of ['pointerup', 'pointerleave', 'pointercancel']) btn.addEventListener(ev, () => release(d));
  });
  root.querySelector('[data-action=bomb]').addEventListener('pointerdown', e => { e.preventDefault(); primary(); });

  canvas.width = W * T;
  canvas.height = H * T;
  startLevel();
  setMode('title');
  requestAnimationFrame(frame);
})();
