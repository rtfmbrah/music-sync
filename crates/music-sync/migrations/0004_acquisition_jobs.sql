CREATE TABLE acquisition_jobs (
    job_id INTEGER PRIMARY KEY REFERENCES jobs(id),
    provider_item_id INTEGER NOT NULL UNIQUE REFERENCES provider_items(id)
) STRICT;

INSERT INTO acquisition_jobs(job_id, provider_item_id)
SELECT jobs.id, provider_items.id
FROM jobs
JOIN provider_items
  ON jobs.idempotency_key =
     'acquire:' || provider_items.provider || ':' || provider_items.provider_item_id
WHERE jobs.kind = 'acquire';
