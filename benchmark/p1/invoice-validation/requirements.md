# Invoice validation

Our own program, written for the P1 benchmark. Both arms of the benchmark get this text and the examples in
`examples.json`; neither sees the oracle or the held-out inputs.

Decide whether a supplier invoice is accepted for payment, list why it is rejected, and say whether an accepted one
still needs a manual approval.

## Inputs

- **Region:** one of the texts `DE`, `NL`, `UK`, `CH`; it fixes the tax rate.
- **Net amount:** the invoice total before tax, a decimal amount from 0.01 to 250000.00 with at most two decimal
  places (whole cents).
- **Declared tax:** the tax the supplier wrote on the invoice, a decimal amount from 0.00 to 60000.00 in whole cents.
- **Line count:** the number of invoice lines, a whole number from 0 to 500.
- **Invoice day** and **received day:** the day the invoice was issued and the day we received it, each a whole day
  number from 1 to 730 (day 1 is the first day of our ledger; a later day has a larger number).
- **Payment terms:** the number of days after the invoice day by which it must be paid, a whole number from 0 to 120.
- **Approver role:** one of the texts `clerk`, `manager`, `director`.

The inputs are independent: any combination within these ranges may occur.

## Calculation

All amounts are exact decimals.

1. **Computed tax** = net amount × the region's rate, rounded to the cent half-even (a remainder of exactly half a
   cent rounds to the even cent: 2.345 becomes 2.34, 2.355 becomes 2.36). Rates: `DE` 19%, `NL` 21%, `UK` 20%,
   `CH` 8.1%.
2. **Gross total** = net amount + computed tax.
3. **Due day** = invoice day + payment terms.

## Rejection rules

Each rule below that fails adds its code to the reasons. The rules are checked independently (one failing does not
skip the others), and the reasons always list the failing codes in this order, the order of the table, each at most
once:

| Code           | The rule fails when                                                                     |
|----------------|-----------------------------------------------------------------------------------------|
| `LINE_COUNT`   | the line count is 0 or more than 200 (1 to 200 lines, both ends included, is fine)       |
| `FUTURE_DATED` | the invoice day is later than the received day (the same day is fine)                    |
| `STALE`        | received day − invoice day is more than 180 (exactly 180 is fine)                       |
| `TERMS`        | the payment terms are more than 60 days (exactly 60 is fine)                             |
| `OVERDUE`      | the due day is earlier than the received day (due on the received day is fine)          |
| `TAX_MISMATCH` | declared tax and computed tax differ by more than 0.02 either way (exactly 0.02 is fine) |
| `OVER_LIMIT`   | the gross total is more than 200000.00 (exactly 200000.00 is fine)                       |

The invoice is **accepted** when no rule fails; then the reasons are an empty list.

**Manual approval** is needed only for an accepted invoice whose gross total is more than the approver's limit:
`clerk` 5000.00, `manager` 50000.00 (a gross total equal to the limit needs none); a `director` never needs one. A
rejected invoice never needs manual approval.

## Output

One record with five fields: **accepted** (true or false), **reasons** (the list of codes above, in table order),
**computed tax** (step 1, a decimal amount), **gross total** (step 2, a decimal amount) and **manual approval** (true
or false).
