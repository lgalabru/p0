# Worked example: programs/iphone-duo

Goal: iPhone Duo 256GB Star White (unlocked), in-store pickup at Apple
Downtown Brooklyn only. Buy it the moment it appears.

## Facts captured from apple.com (2026-11-04)

| | |
| --- | --- |
| Part | `MK1E4LL/A` (`cppart=UNLOCKED/US`, `purchaseOption=fullPrice`) |
| Store | `R715` = Apple Downtown Brooklyn |
| Product page | `/shop/buy-iphone/iphone-duo/7.6-inch-display-256gb-star-white-unlocked` |
| Availability | `GET /shop/fulfillment-messages?fae=true&pl=true&mts.0=regular&parts.0=MK1E4LL/A&store=R715&cppart=UNLOCKED/US&purchaseOption=fullPrice` → `body.content.pickupMessage.stores[storeNumber=R715].partsAvailability["MK1E4LL/A"].pickupDisplay` (`"available"` / `"unavailable"`) |
| Add to bag | `GET <product page>?product=MK1E4LL/A&purchaseOption=fullPrice&cppart=UNLOCKED/US&step=select&acpart=none&bfil=2&atbtoken=<3rd "\|" part of cookie as_atb>&add-to-cart=add-to-cart`, with `redirect: "manual"` |
| Bag count | `GET /shop/bag/status?apikey=SJHJUH4YFCTTPD4F4` → `{items}` |
| Bag → checkout | `button[data-autom=checkout]` → `secure*.store.apple.com/shop/signIn` → `text=Continue as Guest` |
| Remove from bag | `button[data-autom=bag-item-remove-button]` |

## Guest checkout (mapped 2026-11-04)

| Step (`?_s=`) | Act | Proves done |
| --- | --- | --- |
| `signIn` | `text=Continue as Guest` | URL `_s=Fulfillment` |
| `Fulfillment-init` | `text=I’ll pick it up` (curly ’), `input[name=store-locator-result][value=R715]`, `#rs-checkout-continue-button-bottom` | URL `_s=PickupContact` |
| `PickupContact-init` | `[id="checkout.pickupContact.selfPickupContact.selfContact.address.{firstName,lastName,emailAddress,fullDaytimePhone}"]`, continue | URL `_s=Billing` |
| `Billing-init` | `[id="checkout.billing.billingoptions.credit"]`; card `[id="…creditCard.cardInputs.cardInput-0.{cardNumber,expiration,securityCode}"]` (plain inputs, no iframe); billing `[id="…creditCard.billingAddress.address.{firstName,lastName,street,street2,zipLookup.postalCode}"]`; the ZIP fills the `…zipLookup.zipLookupCityState` select; continue | URL `_s=Review` |
| `Review` | Check the total against the cap; click the button matching `/place (your )?order/i` | URL `thankyou` |

The continue button id is shared by every step. Always wait for the next
step's URL or fields before clicking again.

**Server timing.** `signInx` (guest login) and each `continueFrom…` call took
about **9.8 s** server-side in fresh checkout sessions from this Mac. That's
roughly 40 s for the whole checkout, and the browser can't speed it up.
Re-entering an established checkout session answered in 0.4 s. Treat this as
the site's behaviour; don't try to defeat it.

## Structure

- `src/watch.js` runs in the page. It polls availability with AIMD pacing and
  adds to bag in the same turn on a hit, verifies the bag count, and resolves
  `{polls, ms, atbMs, items, quote}`.
- `src/lib.rs` contains:
  - the manifest: contact, card, billing, and an optional Apple ID fallback;
    origins `www.apple.com` and `secure*.store.apple.com`
  - `main`, which runs the watch
  - `checkout`, which goes to the bag, clicks Check Out, then Continue as
    Guest. The remaining steps follow.
- Flags:
  - `--set dry-run` stops before Place Order.
  - `--set force` requires dry-run and treats the item as in stock.
  - `--set interval=<ms>` sets the starting poll gap (default 2500).

## Lessons

- Headless Chrome for Testing gets 541. A real Chrome profile gets 200.
- About 1.4 req/s gets 503 after about 25 s. Retrying through it escalated to
  a session-wide 541 that lasted more than 15 minutes.
- Guest checkout avoids Apple ID session expiry and 2FA.
