//! The audio ports one mapping is routed for (ADR 0382).
//!
//! A helper reports the ports its plugin was activated with. The parent
//! creates each mapping for one port layout, and the helper processes a block
//! only while that layout is the one its plugin was activated with. A mapping,
//! its helper buffers, and the parent's routing therefore change together.

use crate::error::{ProtocolError, SessionError};
use crate::region::SessionLayout;

/// Channel counts of each input port, then each output port, in the plugin's
/// declaration order. The first port of each side is the main port.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortLayout {
    pub inputs: Vec<u32>,
    pub outputs: Vec<u32>,
}

impl PortLayout {
    pub fn new(inputs: Vec<u32>, outputs: Vec<u32>) -> Self {
        Self { inputs, outputs }
    }

    /// Every input channel, flattened in port order.
    pub fn input_channels(&self) -> Result<u32, SessionError> {
        sum_channels(&self.inputs)
    }

    /// Every output channel, flattened in port order.
    pub fn output_channels(&self) -> Result<u32, SessionError> {
        sum_channels(&self.outputs)
    }

    /// `layout` with the channel counts these ports flatten to.
    pub fn fit(&self, layout: SessionLayout) -> Result<SessionLayout, SessionError> {
        Ok(SessionLayout {
            input_channels: self.input_channels()?,
            output_channels: self.output_channels()?,
            ..layout
        })
    }

    /// True when `layout` holds exactly these ports' channels.
    pub fn matches(&self, layout: SessionLayout) -> bool {
        self.fit(layout).is_ok_and(|fitted| fitted == layout)
    }
}

fn sum_channels(counts: &[u32]) -> Result<u32, SessionError> {
    let mut total = 0u32;
    for channels in counts {
        total = total
            .checked_add(*channels)
            .ok_or(ProtocolError::ChannelCountOverflow)?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_holds_exactly_the_flattened_ports() {
        let ports = PortLayout::new(vec![1, 1], vec![2]);
        let stereo = SessionLayout::stereo();
        assert!(ports.matches(stereo));
        assert!(!PortLayout::new(vec![2], vec![1]).matches(stereo));
        let fitted = PortLayout::new(vec![], vec![6]).fit(stereo).unwrap();
        assert_eq!((fitted.input_channels, fitted.output_channels), (0, 6));
        assert_eq!(fitted.input_events, stereo.input_events);
        assert!(matches!(
            PortLayout::new(vec![u32::MAX, 1], vec![]).input_channels(),
            Err(SessionError::Protocol(ProtocolError::ChannelCountOverflow))
        ));
    }
}
