// Checks the Beaver Bomber rules in site/game.js: node scripts/game_test.js
const assert = require('node:assert/strict');
const R = require('../site/game.js');

// Levels: walled border, pillars, a free start corner, enemies away from the start.
for (let level = 1; level <= 8; level++) {
  const { tiles, enemies, items } = R.makeLevel(level, R.rng(level * 7));
  assert.equal(tiles.length, R.H);
  for (let y = 0; y < R.H; y++)
    for (let x = 0; x < R.W; x++) {
      if (x === 0 || y === 0 || x === R.W - 1 || y === R.H - 1 || (x % 2 === 0 && y % 2 === 0)) assert.equal(tiles[y][x], R.WALL);
      if (x + y <= 4 && tiles[y][x] !== R.WALL) assert.equal(tiles[y][x], R.FLOOR, `start corner ${x},${y}`);
    }
  assert.equal(enemies.length, Math.min(2 + level, 9));
  for (const e of enemies) {
    assert.equal(tiles[e.y][e.x], R.FLOOR);
    assert.ok(e.x + e.y >= 8);
  }
  for (const k of Object.keys(items)) {
    const [x, y] = k.split(',').map(Number);
    assert.equal(tiles[y][x], R.CRATE, 'power-ups hide under crates');
  }
}
// Same seed, same level.
assert.deepEqual(R.makeLevel(3, R.rng(42)), R.makeLevel(3, R.rng(42)));
// …and another seed, another level (0 is a seed like any other).
assert.notDeepEqual(R.makeLevel(1, R.rng(0)), R.makeLevel(1, R.rng(1)));

// Blast: stops at walls, takes the first crate and stops there.
const { WALL: X, CRATE: C, FLOOR: o } = R;
const tiles = [
  [X, X, X, X, X, X, X],
  [X, o, o, o, C, o, X],
  [X, o, X, o, X, o, X],
  [X, o, o, o, o, o, X],
  [X, X, X, X, X, X, X],
];
const cells = R.blast(tiles, 3, 1, 3).map(c => c.join(',')).sort();
assert.deepEqual(cells, ['1,1', '2,1', '3,1', '3,2', '3,3', '4,1'].sort());

// Chain reaction: a bomb inside the flames goes off too.
const a = { x: 1, y: 3, range: 2 }, b = { x: 3, y: 3, range: 2 }, far = { x: 5, y: 1, range: 1 };
const out = R.explode(tiles, [a, b, far], a);
assert.deepEqual(new Set(out.bombs), new Set([a, b]));
assert.ok(out.cells.some(([x, y]) => x === 5 && y === 3), "b's flames reach 5,3");

// Walking: floor only, and never onto a bomb.
assert.ok(R.walkable(tiles, [], 1, 1));
assert.ok(!R.walkable(tiles, [], 4, 1));
assert.ok(!R.walkable(tiles, [{ x: 1, y: 1 }], 1, 1));
assert.ok(!R.walkable(tiles, [], 9, 9));

// Sprites: 16x16, only known colours.
for (const [name, rows] of Object.entries(R.ART)) {
  assert.equal(rows.length, 16, name);
  rows.forEach((row, i) => {
    assert.equal(row.length, 16, `${name} row ${i}`);
    for (const ch of row) assert.ok(ch === '.' || R.PAL[ch], `${name} row ${i}: ${ch}`);
  });
}

console.log('game rules: all passed');
