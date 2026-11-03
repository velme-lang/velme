# VAT and shipping

Our own program, written for the P1 benchmark. Both arms of the benchmark get this text and the examples in
`examples.json`; neither sees the oracle or the held-out inputs.

An online shop based in the home country ships one parcel of goods, all of one product category, to a customer in
some region. Work out the shipping charge, the VAT on the goods and on the shipping, and the gross total.

## Inputs

- **Region:** where the parcel goes, one of the texts `home`, `eu`, `uk`, `world`.
- **Category:** the goods' VAT category, one of the texts `standard`, `books`, `food`.
- **Net amount:** the goods' price before VAT, a decimal amount from 0.01 to 5000.00 with at most two decimal places.
- **Weight:** the parcel's weight in kilograms, a decimal from 0.001 to 30.000 with at most three decimal places.

The inputs are independent: any combination within these ranges may occur.

## VAT rates

| Region  | standard | books | food |
|---------|----------|-------|------|
| `home`  | 19%      | 7%    | 7%   |
| `eu`    | 21%      | 6%    | 9%   |
| `uk`    | 20%      | 0%    | 5%   |
| `world` | 0%       | 0%    | 0%   |

## Shipping (net, before VAT)

The weight band is the first one the weight fits: up to and including 0.500 kg; up to and including 2.000 kg; up to
and including 10.000 kg; up to and including 30.000 kg. So 0.500 kg is in the first band and 0.501 kg in the second.

| Region  | to 0.500 kg | to 2.000 kg | to 10.000 kg | to 30.000 kg |
|---------|-------------|-------------|--------------|--------------|
| `home`  | 4.50        | 6.90        | 11.50        | 24.00        |
| `eu`    | 9.50        | 15.00       | 26.00        | 48.00        |
| `uk`    | 12.00       | 19.50       | 34.00        | 65.00        |
| `world` | 18.00       | 29.00       | 55.00        | 110.00       |

Shipping is free (0) for `home` when the net amount is 50.00 or more, and for `eu` when the net amount is 150.00 or
more. It is never free for `uk` or `world`.

## Calculation

"Rounded to the cent" means rounded to two decimal places, half to even (a third decimal of exactly 5, with nothing
after it, rounds to the even cent: 0.285 becomes 0.28 and 0.295 becomes 0.30). Each VAT amount is rounded on its
own.

1. **Goods VAT** = net amount × the VAT rate for the region and category, rounded to the cent.
2. **Shipping VAT** = shipping × the shipping VAT rate, rounded to the cent. The shipping VAT rate is: for `home` and
   `eu`, the same rate as the goods; for `uk`, always 20%, whatever the category; for `world`, 0%.
3. **Total gross** = net amount + shipping + goods VAT + shipping VAT.

## Output

One record with four fields, each a decimal amount: **shipping** (net), **goods VAT**, **shipping VAT** and
**total gross**.
