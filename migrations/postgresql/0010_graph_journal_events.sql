ALTER TABLE research.run_journal
    DROP CONSTRAINT run_journal_event_type_check;

ALTER TABLE research.run_journal
    ADD CONSTRAINT run_journal_event_type_check CHECK (event_type IN (
        'RUN_CREATED', 'RUN_STARTED', 'RUN_SUCCEEDED', 'RUN_FAILED', 'RUN_CANCELLED',
        'ARTIFACT_PUBLISHED', 'SIGNAL_SET_PUBLISHED',
        'NODE_STARTED', 'NODE_SUCCEEDED', 'NODE_FAILED', 'NODE_CHECKPOINTED'
    ));
