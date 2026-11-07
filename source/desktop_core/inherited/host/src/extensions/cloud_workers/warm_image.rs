use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_IMAGE_ID_CHARS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerWarmImageGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerWarmImageState {
    Missing,
    Pending,
    Available,
    Retiring,
    CaptureUncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerWarmImageOperation {
    Allocate,
    Capture,
    Fork,
    Release,
    Retire,
    Delete,
    Recover,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerWarmImage {
    pub image_id: String,
    pub generation: CloudWorkerWarmImageGeneration,
    pub state: CloudWorkerWarmImageState,
    pub hold_count: u32,
    pub cleanup_debt: bool,
}

impl CloudWorkerWarmImage {
    pub fn new(image_id: impl Into<String>) -> Result<Self, CloudWorkerWarmImageError> {
        let image_id = image_id.into();
        if !valid_image_id(&image_id) {
            return Err(CloudWorkerWarmImageError::InvalidImageId);
        }
        Ok(Self {
            image_id,
            generation: CloudWorkerWarmImageGeneration(0),
            state: CloudWorkerWarmImageState::Missing,
            hold_count: 0,
            cleanup_debt: false,
        })
    }

    pub fn compare_and_apply(
        &mut self,
        expected_generation: CloudWorkerWarmImageGeneration,
        operation: CloudWorkerWarmImageOperation,
    ) -> Result<(), CloudWorkerWarmImageError> {
        if self.generation != expected_generation {
            return Err(CloudWorkerWarmImageError::StaleGeneration {
                expected: expected_generation,
                actual: self.generation.clone(),
            });
        }

        let (next, next_hold_count, cleanup_debt) = match (self.state, operation) {
            (CloudWorkerWarmImageState::Missing, CloudWorkerWarmImageOperation::Allocate)
            | (CloudWorkerWarmImageState::CaptureUncertain, CloudWorkerWarmImageOperation::Recover) => {
                (CloudWorkerWarmImageState::Pending, self.hold_count, self.cleanup_debt)
            }
            (CloudWorkerWarmImageState::Pending, CloudWorkerWarmImageOperation::Capture) => {
                (CloudWorkerWarmImageState::Available, self.hold_count, self.cleanup_debt)
            }
            (CloudWorkerWarmImageState::Available, CloudWorkerWarmImageOperation::Fork) => {
                let hold_count = self
                    .hold_count
                    .checked_add(1)
                    .ok_or(CloudWorkerWarmImageError::HoldLimitReached)?;
                (CloudWorkerWarmImageState::Available, hold_count, self.cleanup_debt)
            }
            (CloudWorkerWarmImageState::Available, CloudWorkerWarmImageOperation::Release) => {
                if self.hold_count == 0 {
                    return Err(CloudWorkerWarmImageError::NoActiveHold);
                }
                (CloudWorkerWarmImageState::Available, self.hold_count - 1, self.cleanup_debt)
            }
            (CloudWorkerWarmImageState::Available, CloudWorkerWarmImageOperation::Retire) => {
                if self.hold_count != 0 {
                    return Err(CloudWorkerWarmImageError::HeldImage);
                }
                (CloudWorkerWarmImageState::Retiring, self.hold_count, self.cleanup_debt)
            }
            (CloudWorkerWarmImageState::Retiring, CloudWorkerWarmImageOperation::Delete) => {
                (CloudWorkerWarmImageState::Missing, self.hold_count, self.cleanup_debt)
            }
            (CloudWorkerWarmImageState::CaptureUncertain, CloudWorkerWarmImageOperation::Delete) => {
                (CloudWorkerWarmImageState::Missing, self.hold_count, true)
            }
            _ => {
                return Err(CloudWorkerWarmImageError::InvalidOperation {
                    state: self.state,
                    operation,
                });
            }
        };

        let next_generation = self
            .generation
            .0
            .checked_add(1)
            .ok_or(CloudWorkerWarmImageError::GenerationExhausted)?;
        self.state = next;
        self.hold_count = next_hold_count;
        self.cleanup_debt = cleanup_debt;
        self.generation.0 = next_generation;
        Ok(())
    }

    pub fn validate_persisted(&self) -> Result<(), CloudWorkerWarmImageError> {
        if !valid_image_id(&self.image_id)
            || (self.state != CloudWorkerWarmImageState::Available && self.hold_count != 0)
            || (self.state == CloudWorkerWarmImageState::CaptureUncertain && !self.cleanup_debt)
        {
            return Err(CloudWorkerWarmImageError::InvalidPersistedRecord);
        }
        Ok(())
    }

    pub fn mark_capture_uncertain(
        &mut self,
        expected_generation: CloudWorkerWarmImageGeneration,
    ) -> Result<(), CloudWorkerWarmImageError> {
        if self.generation != expected_generation {
            return Err(CloudWorkerWarmImageError::StaleGeneration {
                expected: expected_generation,
                actual: self.generation.clone(),
            });
        }
        let next_generation = self
            .generation
            .0
            .checked_add(1)
            .ok_or(CloudWorkerWarmImageError::GenerationExhausted)?;
        self.state = CloudWorkerWarmImageState::CaptureUncertain;
        self.cleanup_debt = true;
        self.generation.0 = next_generation;
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerWarmImageError {
    #[error("warm-image id must be non-empty")]
    InvalidImageId,
    #[error("warm-image generation is stale: expected {expected:?}, actual {actual:?}")]
    StaleGeneration {
        expected: CloudWorkerWarmImageGeneration,
        actual: CloudWorkerWarmImageGeneration,
    },
    #[error("warm-image is held by an active fork")]
    HeldImage,
    #[error("warm-image has no active fork hold")]
    NoActiveHold,
    #[error("warm-image hold count exhausted")]
    HoldLimitReached,
    #[error("warm-image generation exhausted")]
    GenerationExhausted,
    #[error("persisted warm-image record is invalid")]
    InvalidPersistedRecord,
    #[error("operation {operation:?} is invalid for state {state:?}")]
    InvalidOperation {
        state: CloudWorkerWarmImageState,
        operation: CloudWorkerWarmImageOperation,
    },
}

fn valid_image_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= MAX_IMAGE_ID_CHARS
        && !value.chars().any(|character| character.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_and_apply_fences_stale_generations() {
        let mut image = CloudWorkerWarmImage::new("image-1").unwrap();
        image
            .compare_and_apply(
                CloudWorkerWarmImageGeneration(0),
                CloudWorkerWarmImageOperation::Allocate,
            )
            .unwrap();
        assert!(matches!(
            image.compare_and_apply(
                CloudWorkerWarmImageGeneration(0),
                CloudWorkerWarmImageOperation::Capture,
            ),
            Err(CloudWorkerWarmImageError::StaleGeneration { .. })
        ));
    }

    #[test]
    fn held_images_cannot_be_retired() {
        let mut image = CloudWorkerWarmImage::new("image-1").unwrap();
        image
            .compare_and_apply(
                CloudWorkerWarmImageGeneration(0),
                CloudWorkerWarmImageOperation::Allocate,
            )
            .unwrap();
        image
            .compare_and_apply(
                CloudWorkerWarmImageGeneration(1),
                CloudWorkerWarmImageOperation::Capture,
            )
            .unwrap();
        image
            .compare_and_apply(
                CloudWorkerWarmImageGeneration(2),
                CloudWorkerWarmImageOperation::Fork,
            )
            .unwrap();
        assert_eq!(
            image.compare_and_apply(
                CloudWorkerWarmImageGeneration(3),
                CloudWorkerWarmImageOperation::Retire,
            ),
            Err(CloudWorkerWarmImageError::HeldImage)
        );
    }

    #[test]
    fn uncertain_capture_is_recoverable_and_records_debt() {
        let mut image = CloudWorkerWarmImage::new("image-1").unwrap();
        image
            .mark_capture_uncertain(CloudWorkerWarmImageGeneration(0))
            .unwrap();
        assert_eq!(image.state, CloudWorkerWarmImageState::CaptureUncertain);
        assert!(image.cleanup_debt);
    }

    #[test]
    fn generation_overflow_fails_before_warm_image_mutation() {
        let mut image = CloudWorkerWarmImage::new("image-1").unwrap();
        image.generation = CloudWorkerWarmImageGeneration(u64::MAX);
        assert_eq!(
            image.compare_and_apply(
                CloudWorkerWarmImageGeneration(u64::MAX),
                CloudWorkerWarmImageOperation::Allocate,
            ),
            Err(CloudWorkerWarmImageError::GenerationExhausted)
        );
        assert_eq!(image.state, CloudWorkerWarmImageState::Missing);
    }

    #[test]
    fn hold_overflow_fails_before_warm_image_mutation() {
        let mut image = CloudWorkerWarmImage::new("image-1").unwrap();
        image.state = CloudWorkerWarmImageState::Available;
        image.hold_count = u32::MAX;
        assert_eq!(
            image.compare_and_apply(
                CloudWorkerWarmImageGeneration(0),
                CloudWorkerWarmImageOperation::Fork,
            ),
            Err(CloudWorkerWarmImageError::HoldLimitReached)
        );
        assert_eq!(image.generation, CloudWorkerWarmImageGeneration(0));
    }

    #[test]
    fn persisted_uncertain_capture_requires_cleanup_debt() {
        let mut image = CloudWorkerWarmImage::new("image-1").unwrap();
        image.state = CloudWorkerWarmImageState::CaptureUncertain;
        assert_eq!(
            image.validate_persisted(),
            Err(CloudWorkerWarmImageError::InvalidPersistedRecord)
        );
        image.cleanup_debt = true;
        assert!(image.validate_persisted().is_ok());
    }
}
