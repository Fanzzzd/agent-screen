//! Closed-control value routes. Family-independent: native and Electron
//! use the same AX role contract. This does not prove an app implements it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueRoute {
    PopupTypeAhead,
    ComboConfirm,
    Attribute,
    RequiresMenu,
    Unavailable,
}

pub fn value_route(role: Option<&str>) -> ValueRoute {
    match role {
        Some("AXPopUpButton") => ValueRoute::PopupTypeAhead,
        Some("AXComboBox") => ValueRoute::ComboConfirm,
        Some("AXMenuButton") => ValueRoute::RequiresMenu,
        None => ValueRoute::Unavailable,
        _ => ValueRoute::Attribute,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_value_routes_depend_on_role_not_app_family() {
        assert_eq!(
            value_route(Some("AXPopUpButton")),
            ValueRoute::PopupTypeAhead
        );
        assert_eq!(value_route(Some("AXComboBox")), ValueRoute::ComboConfirm);
        assert_eq!(value_route(Some("AXMenuButton")), ValueRoute::RequiresMenu);
        assert_eq!(value_route(Some("AXTextField")), ValueRoute::Attribute);
        assert_eq!(value_route(None), ValueRoute::Unavailable);
    }
}
