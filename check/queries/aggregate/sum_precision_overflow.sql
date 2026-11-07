-- expect-error
-- The exact sum exceeds 38 digits but still fits i128: test SQL precision,
-- independently of the wider accumulator's physical conversion bound.
SELECT sum(CAST(7000000000000000000000000000000000000 AS DECIMAL(38,0))) AS s
FROM '{data}/sqlcheck/types.parquet';
