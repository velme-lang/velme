# Order pricing

Our own program, written for the P1 benchmark. Both arms of the benchmark get this text and the examples in
`examples.json`; neither sees the oracle or the held-out inputs.

Price a single-line order: one product, bought in some quantity, by a customer of some tier, with at most one coupon.

## Inputs

- **Customer tier:** one of the texts `standard`, `silver`, `gold`.
- **Unit price:** the product's list price per unit, a decimal amount from 0.50 to 500.00 with at most two decimal
  places (whole cents).
- **Quantity:** the number of units ordered, a whole number from 1 to 1000.
- **Coupon:** one of the texts `NONE`, `SAVE15`, `FLAT25`.

The inputs are independent: any combination within these ranges may occur.

## Calculation

All amounts are exact decimals. "Rounded to the cent" means rounded to two decimal places, half up (a third decimal
of exactly 5 rounds up: 2.345 becomes 2.35). Apply the steps in this order.

1. **List subtotal** = unit price × quantity.
2. **Volume discount** = list subtotal × the volume rate for the quantity, rounded to the cent. Volume rate:
   quantity 1 to 9: 0%; 10 to 49: 4%; 50 to 199: 8%; 200 to 1000: 12% (each range includes both ends).
3. **Tier discount** = (list subtotal − volume discount) × the tier rate, rounded to the cent. Tier rate:
   `standard` 0%, `silver` 3%, `gold` 5%.
4. **Discounted subtotal** = list subtotal − volume discount − tier discount.
5. **Coupon discount**, based on the discounted subtotal:
   - `NONE`: 0.
   - `SAVE15`: if the discounted subtotal is 100.00 or more, 15% of the discounted subtotal rounded to the cent, but
     never more than 40.00; if it is under 100.00, 0.
   - `FLAT25`: 25.00 if the discounted subtotal is 200.00 or more; otherwise 0.
6. **Merchandise total** = discounted subtotal − coupon discount.
7. **Shipping:** 0 for a `gold` customer, whatever the amount. For any other tier, 0 if the merchandise total is
   75.00 or more, otherwise 6.95.
8. **Total** = merchandise total + shipping.

## Output

One record with five fields, each a decimal amount: **volume discount** (step 2), **tier discount** (step 3),
**coupon discount** (step 5), **shipping** (step 7) and **total** (step 8). A discount that does not apply is 0.
