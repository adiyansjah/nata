-- expect-error
SELECT CAST('NaN'::DOUBLE AS DECIMAL(5,2)) AS v
