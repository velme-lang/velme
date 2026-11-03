# Insurance quote

Our own program, written for the P1 benchmark. Both arms of the benchmark get this text and the examples in
`examples.json`; neither sees the oracle or the held-out inputs.

Quote a yearly car insurance premium for one driver and one vehicle.

## Inputs

- **Driver age:** in whole years, from 17 to 90.
- **Vehicle group:** one of the texts `A`, `B`, `C`, `D` (from small cars to high-performance cars).
- **Claims in the last 5 years:** a whole number from 0 to 5.
- **Annual mileage:** expected kilometres driven per year, a whole number from 0 to 60000.
- **Has telematics:** true or false; whether the driver accepts a telematics box in the car.

The inputs are independent: any combination within these ranges may occur.

## Calculation

"Rounded to the cent" means rounded to two decimal places, half up (a third decimal of exactly 5 rounds up: 2.345
becomes 2.35). Apply the steps in this order.

1. **Base rate** by vehicle group and age band (ages 17 to 24, 25 to 64, 65 to 90, each band including both ends):

   | Group | 17-24 | 25-64 | 65-90 |
   |-------|-------|-------|-------|
   | `A`   | 620   | 380   | 420   |
   | `B`   | 780   | 450   | 510   |
   | `C`   | 1050  | 590   | 680   |
   | `D`   | 1600  | 820   | 990   |

2. **Loaded premium** = (base rate × claims factor + mileage loading) × young-driver factor, then rounded to the cent.
   - Claims factor (multiplies): 0 claims 1.00; 1 claim 1.25; 2 claims 1.60; 3, 4 or 5 claims 2.20.
   - Mileage loading (adds a fixed amount): 0 to 20000 km: 0; 20001 to 35000 km: 60.00; 35001 to 60000 km: 150.00.
   - Young-driver factor (multiplies): 1.15 for ages 17 to 20 inclusive; 1.00 otherwise.
3. **Discount amount** = loaded premium × (1 − discount multiplier), rounded to the cent. The discount multiplier is
   the product of 0.90 if the driver has telematics (else 1) and 0.85 if the driver has 0 claims (else 1). So both
   discounts together take 23.5% off, not 25%.
4. **Annual premium** = loaded premium − discount amount, but never below the minimum premium of 300.00 and never
   above the cap of 3500.00: an amount under 300.00 becomes 300.00, an amount over 3500.00 becomes 3500.00, and
   anything from 300.00 to 3500.00 inclusive is kept.
5. **Monthly instalment** = annual premium × 1.05 ÷ 12 (paying monthly costs 5% more), rounded to the cent.

## Output

One record with five fields, each a decimal amount: **base rate**, **loaded premium** (step 2), **discount amount**
(step 3), **annual premium** (step 4) and **monthly instalment** (step 5).
