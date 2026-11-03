-- expect-error-nata: approximate
SELECT 1.5::DECIMAL(5,2) < 1.6::DOUBLE AS v
