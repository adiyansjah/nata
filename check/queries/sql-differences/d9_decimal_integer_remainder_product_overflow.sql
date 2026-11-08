-- expect-error-nata: decimal overflow in `*`: the result does not fit DECIMAL(18,7)
SELECT (-12345678901234567890.12345 % CAST(-2147483648 AS INTEGER))
       * CAST(12345.67 AS DECIMAL(10,2)) AS x
FROM '{data}/sqlcheck/one_row.parquet'
