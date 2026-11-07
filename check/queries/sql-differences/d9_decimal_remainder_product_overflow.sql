-- expect-error-nata: decimal overflow in `*`
SELECT (CAST(-12345678901234567890.12345 AS DECIMAL(25,5)) % 99999999.99)
       * CAST(99999999.99 AS DECIMAL(10,2)) AS x
FROM '{data}/sqlcheck/one_row.parquet' LIMIT 1
