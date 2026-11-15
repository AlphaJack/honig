//! Registered extensions (<https://spec.openapis.org/registry/extension/>) that carry a later version's field into
//! an earlier one: each version step promotes the ones of the version it reaches.

use crate::diag::Diagnostics;
use crate::json::join;
use crate::walk::Obj;

/// Promotes each `(extension, field)` of `obj` to the field unless the object already has it.
pub(super) fn promote(obj: &mut Obj, pairs: &[(&str, &str)], ptr: &str, diags: &mut Diagnostics) {
    for (ext, field) in pairs {
        let Some(v) = obj.remove(*ext) else { continue };
        if obj.contains_key(*field) {
            diags.warning(join(ptr, ext), format!("`{ext}` is dropped: `{field}` is set too"));
        } else {
            obj.insert((*field).to_owned(), v);
        }
    }
}
