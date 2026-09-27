-- Set default settings on first install.
INSERT OR IGNORE INTO settings (key, value, updated_at) VALUES
    ('bind',                '127.0.0.1:7780', strftime('%s','now') * 1000),
    ('download_dir',        '',               strftime('%s','now') * 1000),
    ('segments_default',    '8',              strftime('%s','now') * 1000),
    ('max_concurrent_tasks','3',              strftime('%s','now') * 1000),
    ('max_connections',     '24',             strftime('%s','now') * 1000),
    ('qos_enabled',         'false',          strftime('%s','now') * 1000),
    ('qos_target_bps',      '0',              strftime('%s','now') * 1000),
    ('protocol_pref',       'http2,http3',    strftime('%s','now') * 1000),
    ('user_agent',          'HyprFetch/0.1',  strftime('%s','now') * 1000),
    ('ssrf_block_private',  'true',           strftime('%s','now') * 1000);
