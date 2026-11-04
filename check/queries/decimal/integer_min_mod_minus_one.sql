-- expect-error
SELECT (-128)::TINYINT % (-1)::TINYINT AS v
