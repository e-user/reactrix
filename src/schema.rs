table! {
    datastore (hash) {
        hash -> Bytea,
        data -> Bytea,
    }
}

table! {
    events (sequence) {
        sequence -> Int8,
        version -> Int4,
        data -> Jsonb,
        timestamp -> Timestamptz,
    }
}

table! {
    keystore (hash) {
        hash -> Bytea,
        key -> Bytea,
    }
}

allow_tables_to_appear_in_same_query!(
    datastore,
    events,
    keystore,
);
