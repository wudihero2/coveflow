use std::sync::{Arc, Mutex};

/// Tracks 3D resource pool (CPU, memory, disk) for a worker.
///
/// Two allocation modes:
/// - Normal runs: `try_acquire()` returns an RAII `ResourceGuard` that auto-releases on drop.
/// - Dedicated/Runner Group: `reserve()` returns a `ResourceReservation` for permanent allocation.
pub struct ResourceManager {
    total_cpus: f32,
    total_memory_mb: u64,
    total_disk_mb: u64,
    inner: Mutex<ResourceState>,
    notify: tokio::sync::Notify,
}

struct ResourceState {
    used_cpus: f32,
    used_memory_mb: u64,
    used_disk_mb: u64,
    reserved_cpus: f32,
    reserved_memory_mb: u64,
    reserved_disk_mb: u64,
}

impl ResourceState {
    fn available(
        &self,
        total_cpus: f32,
        total_memory_mb: u64,
        total_disk_mb: u64,
    ) -> (f32, u64, u64) {
        (
            total_cpus - self.reserved_cpus - self.used_cpus,
            total_memory_mb - self.reserved_memory_mb - self.used_memory_mb,
            total_disk_mb - self.reserved_disk_mb - self.used_disk_mb,
        )
    }
}

/// RAII guard for normal run resource allocation. Automatically releases resources on drop.
pub struct ResourceGuard {
    cpus: f32,
    memory_mb: u64,
    disk_mb: u64,
    manager: Arc<ResourceManager>,
}

impl ResourceGuard {
    pub fn manager(&self) -> &Arc<ResourceManager> {
        &self.manager
    }

    pub fn try_resize(&mut self, new_cpus: f32, new_memory_mb: u64, new_disk_mb: u64) -> bool {
        let mut state = self.manager.lock();

        let need_more_cpu = new_cpus > self.cpus;
        let need_more_mem = new_memory_mb > self.memory_mb;
        let need_more_disk = new_disk_mb > self.disk_mb;

        if need_more_cpu || need_more_mem || need_more_disk {
            let (avail_cpus, avail_mem, avail_disk) = state.available(
                self.manager.total_cpus,
                self.manager.total_memory_mb,
                self.manager.total_disk_mb,
            );
            let extra_cpu = new_cpus - self.cpus;
            let extra_mem = new_memory_mb.saturating_sub(self.memory_mb);
            let extra_disk = new_disk_mb.saturating_sub(self.disk_mb);
            if extra_cpu > avail_cpus || extra_mem > avail_mem || extra_disk > avail_disk {
                return false;
            }
        }

        let released =
            new_cpus < self.cpus || new_memory_mb < self.memory_mb || new_disk_mb < self.disk_mb;

        state.used_cpus = (state.used_cpus - self.cpus + new_cpus).max(0.0);
        state.used_memory_mb = state
            .used_memory_mb
            .saturating_sub(self.memory_mb)
            .saturating_add(new_memory_mb);
        state.used_disk_mb = state
            .used_disk_mb
            .saturating_sub(self.disk_mb)
            .saturating_add(new_disk_mb);
        self.cpus = new_cpus;
        self.memory_mb = new_memory_mb;
        self.disk_mb = new_disk_mb;

        if released {
            self.manager.notify.notify_waiters();
        }

        true
    }
}

/// Reservation for dedicated/runner group permanent allocation. Releases resources on drop.
pub struct ResourceReservation {
    cpus: f32,
    memory_mb: u64,
    disk_mb: u64,
    manager: Arc<ResourceManager>,
}

impl ResourceManager {
    #[allow(clippy::expect_used)] // Resource accounting may be inconsistent after poisoning; fail fast.
    fn lock(&self) -> std::sync::MutexGuard<'_, ResourceState> {
        self.inner.lock().expect("ResourceManager lock poisoned")
    }

    pub fn new(cpus: f32, memory_mb: u64, disk_mb: u64) -> Self {
        tracing::info!(
            total_cpus = cpus,
            total_memory_mb = memory_mb,
            total_disk_mb = disk_mb,
            "ResourceManager initialized"
        );
        Self {
            total_cpus: cpus,
            total_memory_mb: memory_mb,
            total_disk_mb: disk_mb,
            inner: Mutex::new(ResourceState {
                used_cpus: 0.0,
                used_memory_mb: 0,
                used_disk_mb: 0,
                reserved_cpus: 0.0,
                reserved_memory_mb: 0,
                reserved_disk_mb: 0,
            }),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// Returns (available_cpus, available_memory_mb, available_disk_mb).
    /// Available = total - reserved - used.
    pub fn available(&self) -> (f32, u64, u64) {
        let state = self.lock();
        state.available(self.total_cpus, self.total_memory_mb, self.total_disk_mb)
    }

    /// Returns (used_cpus, used_memory_mb, used_disk_mb) for metrics reporting.
    pub fn used(&self) -> (f32, u64, u64) {
        let state = self.lock();
        (state.used_cpus, state.used_memory_mb, state.used_disk_mb)
    }

    /// Returns the resolved (total_cpus, total_memory_mb, total_disk_mb). This is
    /// the single source of truth for a worker's advertised capacity.
    pub fn totals(&self) -> (f32, u64, u64) {
        (self.total_cpus, self.total_memory_mb, self.total_disk_mb)
    }

    /// Non-blocking acquire for normal runs. Returns `None` if insufficient resources.
    pub fn try_acquire(
        self: &Arc<Self>,
        cpus: f32,
        memory_mb: u64,
        disk_mb: u64,
    ) -> Option<ResourceGuard> {
        let mut state = self.lock();
        let (avail_cpus, avail_mem, avail_disk) =
            state.available(self.total_cpus, self.total_memory_mb, self.total_disk_mb);

        if cpus > avail_cpus || memory_mb > avail_mem || disk_mb > avail_disk {
            tracing::debug!(
                requested_cpus = cpus,
                requested_memory_mb = memory_mb,
                requested_disk_mb = disk_mb,
                avail_cpus,
                avail_memory_mb = avail_mem,
                avail_disk_mb = avail_disk,
                "try_acquire failed: insufficient resources"
            );
            return None;
        }

        state.used_cpus += cpus;
        state.used_memory_mb += memory_mb;
        state.used_disk_mb += disk_mb;

        tracing::debug!(
            cpus,
            memory_mb,
            disk_mb,
            remaining_cpus = avail_cpus - cpus,
            remaining_memory_mb = avail_mem - memory_mb,
            remaining_disk_mb = avail_disk - disk_mb,
            "try_acquire succeeded"
        );

        Some(ResourceGuard {
            cpus,
            memory_mb,
            disk_mb,
            manager: Arc::clone(self),
        })
    }

    /// Permanent reserve for dedicated/runner group mode. Returns `None` if insufficient resources.
    pub fn reserve(
        self: &Arc<Self>,
        cpus: f32,
        memory_mb: u64,
        disk_mb: u64,
    ) -> Option<ResourceReservation> {
        let mut state = self.lock();
        let (avail_cpus, avail_mem, avail_disk) =
            state.available(self.total_cpus, self.total_memory_mb, self.total_disk_mb);

        if cpus > avail_cpus || memory_mb > avail_mem || disk_mb > avail_disk {
            tracing::debug!(
                requested_cpus = cpus,
                requested_memory_mb = memory_mb,
                requested_disk_mb = disk_mb,
                avail_cpus,
                avail_memory_mb = avail_mem,
                avail_disk_mb = avail_disk,
                "reserve failed: insufficient resources"
            );
            return None;
        }

        state.reserved_cpus += cpus;
        state.reserved_memory_mb += memory_mb;
        state.reserved_disk_mb += disk_mb;

        tracing::debug!(
            cpus,
            memory_mb,
            disk_mb,
            remaining_cpus = avail_cpus - cpus,
            remaining_memory_mb = avail_mem - memory_mb,
            remaining_disk_mb = avail_disk - disk_mb,
            "reserve succeeded"
        );

        Some(ResourceReservation {
            cpus,
            memory_mb,
            disk_mb,
            manager: Arc::clone(self),
        })
    }

    /// Async wait until any resource is released (guard or reservation dropped).
    pub async fn wait_for_release(&self) {
        self.notify.notified().await;
    }
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        let mut state = self.manager.lock();
        // Clamp on release: repeated f32 acquire/release cycles leave residual
        // drift, and the subtraction must never push `used` negative (which would
        // make available() report more than the worker actually has). u64 dims use
        // saturating_sub to avoid an underflow panic on any double-release path.
        state.used_cpus = (state.used_cpus - self.cpus).max(0.0);
        state.used_memory_mb = state.used_memory_mb.saturating_sub(self.memory_mb);
        state.used_disk_mb = state.used_disk_mb.saturating_sub(self.disk_mb);

        tracing::debug!(
            cpus = self.cpus,
            memory_mb = self.memory_mb,
            disk_mb = self.disk_mb,
            "ResourceGuard released"
        );

        self.manager.notify.notify_waiters();
    }
}

impl Drop for ResourceReservation {
    fn drop(&mut self) {
        let mut state = self.manager.lock();
        state.reserved_cpus -= self.cpus;
        state.reserved_memory_mb -= self.memory_mb;
        state.reserved_disk_mb -= self.disk_mb;

        tracing::debug!(
            cpus = self.cpus,
            memory_mb = self.memory_mb,
            disk_mb = self.disk_mb,
            "ResourceReservation released"
        );

        self.manager.notify.notify_waiters();
    }
}

impl ResourceReservation {
    /// Explicit release that consumes self, triggering Drop.
    pub fn release(self) {
        // Drop runs automatically when self is consumed.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager(cpus: f32, mem: u64, disk: u64) -> Arc<ResourceManager> {
        Arc::new(ResourceManager::new(cpus, mem, disk))
    }

    #[test]
    fn test_new_and_available() {
        let mgr = make_manager(4.0, 8192, 102400);
        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 4.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192);
        assert_eq!(disk, 102400);
    }

    #[test]
    fn test_try_acquire_success() {
        let mgr = make_manager(4.0, 8192, 102400);
        let guard = mgr.try_acquire(1.0, 512, 1024);
        assert!(guard.is_some());

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 3.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 512);
        assert_eq!(disk, 102400 - 1024);
    }

    #[test]
    fn test_try_acquire_insufficient_cpu() {
        let mgr = make_manager(2.0, 8192, 102400);
        let guard = mgr.try_acquire(3.0, 512, 1024);
        assert!(guard.is_none());

        // Resources unchanged
        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 2.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192);
        assert_eq!(disk, 102400);
    }

    #[test]
    fn test_try_acquire_insufficient_memory() {
        let mgr = make_manager(4.0, 512, 102400);
        let guard = mgr.try_acquire(1.0, 1024, 1024);
        assert!(guard.is_none());
    }

    #[test]
    fn test_try_acquire_insufficient_disk() {
        let mgr = make_manager(4.0, 8192, 500);
        let guard = mgr.try_acquire(1.0, 512, 1024);
        assert!(guard.is_none());
    }

    #[test]
    fn test_guard_drop_releases_resources() {
        let mgr = make_manager(4.0, 8192, 102400);

        {
            let _guard = mgr.try_acquire(2.0, 4096, 51200).unwrap();
            let (cpus, mem, disk) = mgr.available();
            assert!((cpus - 2.0).abs() < f32::EPSILON);
            assert_eq!(mem, 4096);
            assert_eq!(disk, 51200);
        }
        // Guard dropped

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 4.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192);
        assert_eq!(disk, 102400);
    }

    #[test]
    fn test_multiple_acquires() {
        let mgr = make_manager(4.0, 8192, 102400);
        let _g1 = mgr.try_acquire(1.0, 1024, 10240).unwrap();
        let _g2 = mgr.try_acquire(1.5, 2048, 20480).unwrap();

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 1.5).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 1024 - 2048);
        assert_eq!(disk, 102400 - 10240 - 20480);

        // Third acquire should fail (not enough CPU)
        let g3 = mgr.try_acquire(2.0, 512, 1024);
        assert!(g3.is_none());
    }

    #[test]
    fn test_reserve_success() {
        let mgr = make_manager(4.0, 8192, 102400);
        let reservation = mgr.reserve(2.0, 4096, 51200);
        assert!(reservation.is_some());

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 2.0).abs() < f32::EPSILON);
        assert_eq!(mem, 4096);
        assert_eq!(disk, 51200);
    }

    #[test]
    fn test_reserve_reduces_available() {
        let mgr = make_manager(4.0, 8192, 102400);
        let _reservation = mgr.reserve(3.0, 6144, 81920).unwrap();

        // try_acquire should only have the unreserved portion available
        let guard = mgr.try_acquire(2.0, 2048, 20480);
        assert!(guard.is_none()); // Only 1.0 CPU / 2048 MB / 20480 MB free

        let guard = mgr.try_acquire(1.0, 2048, 20480);
        assert!(guard.is_some());
    }

    #[test]
    fn test_reservation_drop_releases() {
        let mgr = make_manager(4.0, 8192, 102400);

        {
            let _reservation = mgr.reserve(2.0, 4096, 51200).unwrap();
            let (cpus, _, _) = mgr.available();
            assert!((cpus - 2.0).abs() < f32::EPSILON);
        }
        // Reservation dropped

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 4.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192);
        assert_eq!(disk, 102400);
    }

    #[test]
    fn test_mixed_acquire_and_reserve() {
        let mgr = make_manager(4.0, 8192, 102400);

        let _reservation = mgr.reserve(1.0, 2048, 20480).unwrap();
        let _guard = mgr.try_acquire(2.0, 4096, 40960).unwrap();

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 1.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 2048 - 4096);
        assert_eq!(disk, 102400 - 20480 - 40960);

        // Should fail — only 1.0 CPU left
        assert!(mgr.try_acquire(1.5, 512, 1024).is_none());
        // Should succeed — exactly 1.0 CPU left
        assert!(mgr.try_acquire(1.0, 512, 1024).is_some());
    }

    #[tokio::test]
    async fn test_notify_on_release() {
        let mgr = make_manager(1.0, 512, 1024);
        let guard = mgr.try_acquire(1.0, 512, 1024).unwrap();

        // Fully exhausted — next acquire fails
        assert!(mgr.try_acquire(0.5, 256, 512).is_none());

        let mgr2 = Arc::clone(&mgr);
        let handle = tokio::spawn(async move {
            mgr2.wait_for_release().await;
            // After wakeup, resources should be available again
            let (cpus, mem, disk) = mgr2.available();
            assert!((cpus - 1.0).abs() < f32::EPSILON);
            assert_eq!(mem, 512);
            assert_eq!(disk, 1024);
        });

        // Small yield to ensure the spawned task registers the notified future
        tokio::task::yield_now().await;

        drop(guard);
        handle.await.unwrap();
    }

    #[test]
    fn test_try_resize_expand_success() {
        let mgr = make_manager(4.0, 8192, 102400);
        let mut guard = mgr.try_acquire(0.1, 1, 1).unwrap();
        assert!(guard.try_resize(1.0, 512, 1024));

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 3.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 512);
        assert_eq!(disk, 102400 - 1024);
    }

    #[test]
    fn test_try_resize_expand_insufficient() {
        let mgr = make_manager(2.0, 1024, 2048);
        let _g1 = mgr.try_acquire(1.5, 512, 1024).unwrap();
        let mut g2 = mgr.try_acquire(0.1, 1, 1).unwrap();

        // Only 0.4 CPU left, can't resize to 1.0
        assert!(!g2.try_resize(1.0, 512, 1024));

        // Guard should still hold original values
        let (cpus, _, _) = mgr.available();
        assert!((cpus - (2.0 - 1.5 - 0.1)).abs() < 0.01);
    }

    #[test]
    fn test_try_resize_shrink() {
        let mgr = make_manager(4.0, 8192, 102400);
        let mut guard = mgr.try_acquire(2.0, 4096, 51200).unwrap();
        assert!(guard.try_resize(0.5, 256, 512));

        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 3.5).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 256);
        assert_eq!(disk, 102400 - 512);
    }

    #[test]
    fn test_try_resize_shrink_then_drop() {
        let mgr = make_manager(4.0, 8192, 102400);
        {
            let mut guard = mgr.try_acquire(2.0, 4096, 51200).unwrap();
            assert!(guard.try_resize(0.5, 256, 512));
        }
        // After drop, all resources restored
        let (cpus, mem, disk) = mgr.available();
        assert!((cpus - 4.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192);
        assert_eq!(disk, 102400);
    }

    #[tokio::test]
    async fn test_try_resize_shrink_notifies() {
        let mgr = make_manager(1.0, 512, 1024);
        let mut guard = mgr.try_acquire(1.0, 512, 1024).unwrap();

        // Fully exhausted
        assert!(mgr.try_acquire(0.1, 1, 1).is_none());

        let mgr2 = Arc::clone(&mgr);
        let handle = tokio::spawn(async move {
            mgr2.wait_for_release().await;
            let (cpus, _, _) = mgr2.available();
            assert!(cpus > 0.0);
        });

        tokio::task::yield_now().await;

        // Shrink should notify waiters
        assert!(guard.try_resize(0.5, 256, 512));
        handle.await.unwrap();
    }
}
