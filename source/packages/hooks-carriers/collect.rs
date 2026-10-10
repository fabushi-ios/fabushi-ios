pub fn append_hook_additional_contexts<T: Clone>(
    collector: Option<&mut Vec<T>>,
    contexts: &[T],
) {
    if contexts.is_empty() {
        return;
    }
    if let Some(collector) = collector {
        collector.extend_from_slice(contexts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_only_when_collector_exists_and_contexts_are_nonempty() {
        let mut collector = vec![1];
        append_hook_additional_contexts(Some(&mut collector), &[2, 3]);
        assert_eq!(collector, vec![1, 2, 3]);

        append_hook_additional_contexts::<i32>(None, &[4]);
        append_hook_additional_contexts(Some(&mut collector), &[]);
        assert_eq!(collector, vec![1, 2, 3]);
    }
}
