-- expect-error-nata: division by zero
SELECT 1.00::DECIMAL(15,2) / 0::DECIMAL(15,2) AS v
