-- R04: make revision/event/outbox membership enforceable by PostgreSQL.
ALTER TABLE article_change_events
    ADD CONSTRAINT uq_article_change_events_event_article_revision
    UNIQUE (event_id, article_id, article_revision);

ALTER TABLE article_change_events
    ADD CONSTRAINT fk_article_change_events_revision
    FOREIGN KEY (article_id, article_revision)
    REFERENCES article_revisions (article_id, revision)
    ON DELETE RESTRICT;

ALTER TABLE delivery_outbox
    ADD CONSTRAINT fk_delivery_outbox_event_revision
    FOREIGN KEY (event_id, article_id, article_revision)
    REFERENCES article_change_events (event_id, article_id, article_revision)
    ON DELETE RESTRICT;
