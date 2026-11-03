# Vacation days

Source: DMN TCK `compliance-level-3/0020-vacation-days`, restated in plain English. Both arms of the benchmark get
this text and the examples in `examples.json`; neither sees the DMN model, the oracle or the held-out inputs.

## Inputs

- **Age:** the employee's age in whole years, from 16 to 80.
- **Years of service:** the employee's completed years of service, a whole number from 0 to 60.

The two inputs are independent: any age may come with any years of service in its range.

## Output

One vacation record with five fields, computed as below. All values are whole numbers of days.

1. **Base vacation days:** always 22.
2. **Extra days case 1:** 5 if the age is under 18, or the age is 60 or over, or the years of service are 30 or
   more; otherwise 0.
3. **Extra days case 2:** 3 if the years of service are 30 or more, or the age is 60 or over; otherwise 0.
4. **Extra days case 3:** 2 if the years of service are 15 or more but under 30, or the age is 45 or over;
   otherwise 0.
5. **Total vacation days:** base vacation days, plus the larger of extra days case 1 and extra days case 3, plus
   extra days case 2.

Each extra-days value is 0 or its single amount: meeting more than one of its conditions does not add it twice.
