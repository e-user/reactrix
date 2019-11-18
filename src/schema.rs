table! {
    events (sequence) {
        sequence -> Int8,
        version -> Int4,
        #[sql_name = "type"]
        type_ -> Text,
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
    events,
    keystore,
);
