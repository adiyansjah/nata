SELECT min(a) AS lo, max(a) AS hi, median(a) AS mid FROM (VALUES (1.10::DECIMAL(15,2)), (2.20::DECIMAL(15,2)), (3.30::DECIMAL(15,2))) t(a)
