# fixed-point-arithmetic Spec

## FixedPrice

### Creation and boundaries

**Given** a raw u32 value within range 0..=10,000
**When** `FixedPrice::new(raw)` is called
**Then** it returns `Ok(FixedPrice)` with the given raw value

**Given** a raw u32 value of 10,001 or greater
**When** `FixedPrice::new(raw)` is called
**Then** it returns `Err(TypesError::InvalidPrice(raw))`

**Given** a raw value of 0
**When** `FixedPrice::new(0)` is called
**Then** it returns `Ok(FixedPrice(0))` equal to `FixedPrice::ZERO`

**Given** a raw value of 10,000
**When** `FixedPrice::new(10_000)` is called
**Then** it returns `Ok(FixedPrice(10_000))` equal to `FixedPrice::ONE`

### f64 conversion

**Given** an f64 value of 0.5
**When** `FixedPrice::from_f64(0.5)` is called
**Then** it returns `Ok(FixedPrice(5000))`

**Given** a `FixedPrice(5000)`
**When** `as_f64()` is called
**Then** it returns 0.5 within f64::EPSILON tolerance

**Given** an f64 value of 1.5 (out of range)
**When** `FixedPrice::from_f64(1.5)` is called
**Then** it returns `Err(TypesError::InvalidPrice(_))`

### String parsing

**Given** a string `"0.1234"`
**When** `FixedPrice::try_from("0.1234")` is called
**Then** it returns `Ok(FixedPrice(1234))`

**Given** a non-numeric string `"abc"`
**When** `FixedPrice::try_from("abc")` is called
**Then** it returns `Err(TypesError::PriceParse(_))`

### Display

**Given** a `FixedPrice(5000)`
**When** formatted with `Display`
**Then** the output is `"0.5000"` (4 decimal places)

### Serde roundtrip

**Given** a `FixedPrice(5000)`
**When** serialized to JSON and deserialized back
**Then** the JSON representation is `"0.5000"` and the deserialized value equals the original

### Ordering

**Given** `FixedPrice::from_f64(0.3)` and `FixedPrice::from_f64(0.7)`
**When** compared with `<`
**Then** the 0.3 price is less than the 0.7 price

**Given** two `FixedPrice` values
**When** used as BTreeMap keys
**Then** they are ordered by their raw u32 value (natural price ordering)

## FixedSize

### Creation and f64 conversion

**Given** an f64 value of 123.456789
**When** `FixedSize::from_f64(123.456789)` is called
**Then** the raw value is 123,456,789

**Given** a `FixedSize` with raw value 123,456,789
**When** `as_f64()` is called
**Then** it returns 123.456789 within 1e-6 tolerance

### String parsing

**Given** a string `"100.5"`
**When** `FixedSize::try_from("100.5")` is called
**Then** the raw value is 100,500,000

**Given** a non-numeric string
**When** `FixedSize::try_from("xyz")` is called
**Then** it returns `Err(TypesError::SizeParse(_))`

### Display

**Given** a `FixedSize::from_f64(10.5)`
**When** formatted with `Display`
**Then** the output is `"10.500000"` (6 decimal places)

### Serde roundtrip

**Given** a `FixedSize::from_f64(10.0)`
**When** serialized to JSON and deserialized back
**Then** the deserialized value equals the original

### Zero check

**Given** `FixedSize::ZERO`
**When** `is_zero()` is called
**Then** it returns true

**Given** `FixedSize::from_f64(1.0)`
**When** `is_zero()` is called
**Then** it returns false
