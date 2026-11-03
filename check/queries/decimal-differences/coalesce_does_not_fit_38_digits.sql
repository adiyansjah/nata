-- expect-error-nata: common type
SELECT coalesce(NULL::DECIMAL(36,2), 1.1234567891::DECIMAL(38,10)) AS v
