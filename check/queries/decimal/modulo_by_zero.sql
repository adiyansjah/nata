-- expect-error
SELECT 5.5::DECIMAL(5,1) % 0::DECIMAL(5,0) AS v
