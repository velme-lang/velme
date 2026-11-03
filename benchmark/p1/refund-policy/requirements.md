# Refund policy

Our own program, written for the P1 benchmark. Both arms of the benchmark get this text and the examples in
`examples.json`; neither sees the oracle or the held-out inputs.

Decide the refund for one returned item: its type, the amount, any restocking fee, and the return window that applied.

## Inputs

- **Category:** one of the texts `electronics`, `apparel`, `furniture`.
- **Membership:** the customer's tier, one of the texts `none`, `plus`, `premier`.
- **Days since purchase:** a whole number from 0 to 365 (0 is a return on the day of purchase).
- **Condition:** one of the texts `unopened`, `opened` (packaging opened, item not used), `used`.
- **Defective:** true or false; whether the item has a manufacturing fault.
- **Price:** what the customer paid for the item, a decimal amount from 1.00 to 5000.00 in whole cents.

The inputs are independent: any combination within these ranges may occur.

## Rules

All amounts are exact decimals. "Rounded to the cent" means rounded to two decimal places, half up (a remainder of
exactly half a cent rounds up: 2.345 becomes 2.35).

**Return window** = the category's days plus the membership's extra days. Category: `electronics` 30, `apparel` 60,
`furniture` 14. Extra: `none` 0, `plus` 15, `premier` 30. A return is *within the window* when the days since purchase
are at most the window (the last day is included), and *in the grace period* when they are more than the window but
at most the window + 30.

Apply the first case that matches, in this order:

1. **Defective, within warranty:** a defective item returned at most 180 days after purchase (day 180 included) gets
   a `full` refund of the price, whatever its condition or window. After 180 days a defective item is handled by the
   cases below as if it were not defective.
2. **Within the window:**
   - `unopened`: a `full` refund of the price.
   - `opened`: refund the price minus a restocking fee. Fee = price × fee rate, rounded to the cent, then capped: never
     more than 75.00. Fee rate: `electronics` 15%, `furniture` 20%, `apparel` 0%; a `plus` member pays half the rate
     (7.5% and 10%), a `premier` member pays none. The refund type is `full` when the fee is 0, else `partial`.
   - `used`: `store_credit` of 50% of the price, rounded to the cent; except `apparel`, which gets `none`.
3. **In the grace period:** an `unopened` or `opened` item gets `store_credit` of 80% of the price, rounded to the
   cent (no restocking fee); a `used` item gets `none`.
4. **Otherwise:** `none`.

## Output

One record with four fields: **refund type** (one of the texts `full`, `partial`, `store_credit`, `none`), **amount**
(the cash refund for `full` and `partial`, the store credit for `store_credit`, 0 for `none`), **restocking fee** (the
fee charged in case 2 `opened`, else 0) and **window days** (the return window above, a whole number, given whatever
case applied).
