INSERT INTO events(level, component, event, message, context_json)
SELECT 'info', 'migration', 'lyrics_signature_mismatch_reclassified',
       'reclassified a prior exact-signature lyrics mismatch as durably unavailable',
       json_object(
         'recording_id', lyrics_resolutions.recording_id,
         'prior_state', lyrics_resolutions.state,
         'prior_message', lyrics_resolutions.message
       )
FROM lyrics_resolutions
WHERE state = 'deferred'
  AND message = 'LRCLIB result contradicts the canonical request signature'
  AND NOT EXISTS (
    SELECT 1 FROM lyrics_selections
    WHERE lyrics_selections.recording_id = lyrics_resolutions.recording_id
  )
  AND NOT EXISTS (
    SELECT 1 FROM lyrics_outputs
    WHERE lyrics_outputs.recording_id = lyrics_resolutions.recording_id
  );

UPDATE lyrics_resolutions
SET state = 'unavailable',
    message = 'LRCLIB returned a result that contradicted the exact request signature',
    updated_at = CURRENT_TIMESTAMP
WHERE state = 'deferred'
  AND message = 'LRCLIB result contradicts the canonical request signature'
  AND NOT EXISTS (
    SELECT 1 FROM lyrics_selections
    WHERE lyrics_selections.recording_id = lyrics_resolutions.recording_id
  )
  AND NOT EXISTS (
    SELECT 1 FROM lyrics_outputs
    WHERE lyrics_outputs.recording_id = lyrics_resolutions.recording_id
  );
