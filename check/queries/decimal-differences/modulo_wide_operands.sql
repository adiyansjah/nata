-- expect-error-nata: aligning the operands
SELECT 1::DECIMAL(18,15) % 1::DECIMAL(30,5) AS v
