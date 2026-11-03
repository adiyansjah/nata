-- tolerance: 1e-12
SELECT stddev(a) AS v FROM (VALUES (1.0::DECIMAL(5,1)), (2.0::DECIMAL(5,1)), (4.0::DECIMAL(5,1))) t(a)
