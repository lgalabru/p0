// Runs entirely in the apple.com page: poll pickup availability, and the instant
// it flips to "available", add to bag in the same JS turn. Zero host round-trips
// on the hot path. Resolves once (with stats) when the item is in the bag.
(async (cfg) => {
  const L = (...a) => console.log("[p0]", ...a);
  const q =
    `/shop/fulfillment-messages?fae=true&pl=true&mts.0=regular&parts.0=${encodeURIComponent(cfg.part)}` +
    `&store=${cfg.store}&cppart=${encodeURIComponent(cfg.cppart)}&purchaseOption=fullPrice`;

  const atbToken = () => {
    const m = document.cookie.match(/(?:^|; )as_atb=([^;]+)/);
    return m ? decodeURIComponent(m[1]).split("|")[2] : null;
  };
  const bagItems = async () =>
    (await (await fetch("/shop/bag/status?apikey=" + cfg.bagApiKey, { cache: "no-store" })).json()).items;

  const addToBag = async () => {
    const tok = atbToken();
    if (!tok) throw new Error("no as_atb cookie (open a product page first)");
    const u =
      `${cfg.productPath}?product=${encodeURIComponent(cfg.part)}&purchaseOption=fullPrice` +
      `&cppart=${encodeURIComponent(cfg.cppart)}&step=select&acpart=none&bfil=2&atbtoken=${tok}&add-to-cart=add-to-cart`;
    // The add happens on this GET (302 → attach page). Don't follow: saves ~1.5s of HTML.
    await fetch(u, { credentials: "include", cache: "no-store", redirect: "manual" });
    return bagItems();
  };

  if (cfg.skipIfInBag && (await bagItems()) > 0) {
    L("bag already has items; skipping watch");
    return { polls: 0, ms: 0, already: true };
  }

  let polls = 0, inflight = 0, done = false, missingStore = false;
  // AIMD pacing: find the fastest rate Apple tolerates and sit just under it.
  let gap = cfg.interval, pauseUntil = 0, okStreak = 0, limited = 0;
  const t0 = performance.now();

  return await new Promise((resolve, reject) => {
    const hit = async (quote) => {
      done = true;
      const tHit = performance.now();
      L(`AVAILABLE after ${polls} polls: ${quote}`);
      try {
        const items = await addToBag();
        const atbMs = Math.round(performance.now() - tHit);
        if (!(items > 0)) return reject(new Error("add-to-bag did not stick (bag empty)"));
        resolve({ polls, ms: Math.round(tHit - t0), atbMs, items, quote });
      } catch (e) {
        reject(e);
      }
    };

    const tick = async () => {
      if (done || inflight >= cfg.conc) return;
      inflight++;
      polls++;
      try {
        const r = await fetch(q + "&_=" + Date.now(), { cache: "no-store", credentials: "include" });
        if (r.status !== 200) {
          // 503 = rate limit (soft). 541 = Akamai bot block (hard, whole session).
          // Hammering through either only extends the block, so back off hard.
          limited++;
          okStreak = 0;
          if (performance.now() > pauseUntil) {
            const hard = r.status === 541;
            gap = Math.min(gap * (hard ? 2 : 1.5), cfg.maxInterval);
            const pause = hard ? cfg.blockPauseMs : cfg.pauseMs;
            pauseUntil = performance.now() + pause;
            L(`http ${r.status}; pause ${pause}ms, gap now ${Math.round(gap)}ms`);
          }
          return;
        }
        if (++okStreak % 20 === 0 && gap > cfg.interval) gap = Math.max(gap * 0.95, cfg.interval);
        const j = await r.json();
        const st = (j.body?.content?.pickupMessage?.stores || []).find((s) => s.storeNumber === cfg.store);
        if (!st) {
          if (!missingStore) L(`store ${cfg.store} missing from response`);
          missingStore = true;
          return;
        }
        const pa = st.partsAvailability?.[cfg.part];
        if (!done && (cfg.force || pa?.pickupDisplay === "available")) {
          hit(pa?.messageTypes?.regular?.storePickupQuote || pa?.pickupDisplay);
        }
      } catch (e) {
        L("poll error", String(e));
      } finally {
        inflight--;
      }
    };

    // Staggered, overlapping requests: with conc=2 and interval=I, a stock flip is
    // observed within ~I after it hits the edge, even though each request takes ~350ms.
    const loop = () => {
      if (done) return;
      const wait = pauseUntil - performance.now();
      if (wait > 0) return setTimeout(loop, wait);
      tick();
      setTimeout(loop, gap * (0.85 + Math.random() * 0.3));
    };
    loop();
    const alive = setInterval(() => (done ? clearInterval(alive) : L(`watching: ${polls} polls, ${limited} limited, gap ${Math.round(gap)}ms`)), cfg.heartbeatMs || 60000);
  });
})(__CFG__)
