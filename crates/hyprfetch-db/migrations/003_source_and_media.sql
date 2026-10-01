-- 003_source_and_media.sql — v0.6.1 media engine + browser extension
--
-- Adds two columns to `tasks`:
--
--   source       Where the task came from:
--                  'app'       — typed/pasted in the WebUI or CLI (default)
--                  'extension' — sent in by the browser extension
--                  'media'     — media-engine (yt-dlp) download with a
--                                selected quality (YouTube, etc.)
--
--   media_meta   JSON blob describing a media-engine download:
--                  { "format_id": "137+140", "quality": "1080p",
--                    "container": "mp4", "extractor": "youtube",
--                    "title": "...", "audio_only": false }
--                NULL for plain HTTP tasks.

ALTER TABLE tasks ADD COLUMN source TEXT NOT NULL DEFAULT 'app';
ALTER TABLE tasks ADD COLUMN media_meta TEXT;

CREATE INDEX IF NOT EXISTS idx_tasks_source ON tasks(source);
