import { test, expect } from '@playwright/test';
import {
  PAGE_LOCK_MS, PAGE_THRESHOLD_PX,
  newAccumulator, resetAccumulator, swipePage, wheelPage,
} from '../src/lib/periodscroll';
import type { PanSample } from '../src/lib/weekwindow';

// The samples a swipe handler gathers: incremental vertical travel, px, stamped
// on `performance.now()`'s clock. `velocityOf` sums the field called `days`,
// which for this gesture carries pixels — see `swipePage`'s comment.
const samples = (...px: number[]): PanSample[] =>
  px.map((d, i) => ({ t: i * 16, days: d }));

test.describe('the wheel page accumulator', () => {
  test('one mouse notch earns exactly one page', () => {
    const r = wheelPage(newAccumulator(), 100, 1000);
    expect(r.page).toBe(1);
    // Spent, not carried: the next notch must still be a whole threshold.
    expect(r.acc.acc).toBe(0);
  });

  test('down is forward, up is back', () => {
    expect(wheelPage(newAccumulator(), 100, 0).page).toBe(1);
    expect(wheelPage(newAccumulator(), -100, 0).page).toBe(-1);
  });

  test('small trackpad events gather, and only cross on the threshold', () => {
    let a = newAccumulator();
    const first = wheelPage(a, 20, 0);
    expect(first.page).toBe(0);
    a = first.acc;
    const second = wheelPage(a, 20, 16);
    expect(second.page).toBe(0); // 40 < 50
    a = second.acc;
    const third = wheelPage(a, 20, 32);
    expect(third.page).toBe(1); // 60 >= 50
  });

  test('the lock swallows the momentum tail, then a fresh flick pages again', () => {
    const p = wheelPage(newAccumulator(), 100, 1000);
    expect(p.page).toBe(1);
    // The tail: seconds of events the trackpad emits after the hand has left.
    const tail = wheelPage(p.acc, 100, 1000 + PAGE_LOCK_MS - 50);
    expect(tail.page).toBe(0);
    expect(tail.acc).toEqual(p.acc);
    const tail2 = wheelPage(tail.acc, 100, 1000 + PAGE_LOCK_MS - 10);
    expect(tail2.page).toBe(0);
    // After the lock, a deliberate new flick earns its page.
    const after = wheelPage(tail2.acc, 100, 1000 + PAGE_LOCK_MS + 10);
    expect(after.page).toBe(1);
  });

  test('a slow drag cannot bank travel across gestures', () => {
    let a = wheelPage(newAccumulator(), PAGE_THRESHOLD_PX - 10, 0).acc;
    a = resetAccumulator(a);
    // The same sub-threshold travel again would otherwise add up to a page.
    expect(wheelPage(a, PAGE_THRESHOLD_PX - 10, 10).page).toBe(0);
  });
});

test.describe('the swipe page decision', () => {
  test('a strong, far-enough swipe pages; direction follows the finger', () => {
    expect(swipePage(-80, samples(-40, -40))).toBe(1); // finger up = forward
    expect(swipePage(80, samples(40, 40))).toBe(-1); // finger down = back
  });

  test('too gentle or too short does nothing', () => {
    // Fast but a twitch: under the 40px floor.
    expect(swipePage(-20, samples(-20, -10))).toBe(0);
    // Far but slow: a deliberate drag, not a flick.
    expect(swipePage(-60, samples(-3, -2, -2, -3))).toBe(0);
  });
});
