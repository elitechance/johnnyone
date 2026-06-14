UPDATE settings
SET value = 'https://johnnyone.ethan-353.workers.dev'
WHERE key = 'worker_url' AND value IN (
  'https://johnnyone-hub.ethan-353.workers.dev',
  'https://johnnyone-dev-hub.ethan-353.workers.dev'
);