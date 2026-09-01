UPDATE genre_resolutions
SET state='deferred',
    message='released for MusicBrainz artist-genre fallback introduced by schema 23',
    updated_at=CURRENT_TIMESTAMP
WHERE state IN ('unavailable','ambiguous')
  AND NOT EXISTS (
      SELECT 1 FROM recording_genres
      WHERE recording_genres.recording_id=genre_resolutions.recording_id
  );

INSERT INTO events(level,component,event,message,context_json)
SELECT 'info','genre','genre_artist_fallback_released',
       'prior terminal genre resolutions released for new MusicBrainz artist fallback',
       json_object('recordings', changes())
WHERE changes()>0;
