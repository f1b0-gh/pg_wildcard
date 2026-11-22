// SPDX-License-Identifier: Apache-2.0
//! Prevent PostgreSQL's process signal handlers from running on Tokio workers.
//!
//! Block before spawning, rather than in an on-thread-start callback: threads
//! inherit the mask at creation, closing the startup window. Blocking DNS work
//! is submitted from these workers and inherits the same mask.

struct SignalMask(libc::sigset_t);

impl SignalMask {
    fn block() -> Result<Self, &'static str> {
        // SAFETY: both masks are valid stack storage. The POSIX functions touch
        // only this thread's signal mask and do not invoke PostgreSQL APIs.
        unsafe {
            let mut blocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            if libc::sigfillset(blocked.as_mut_ptr()) != 0 {
                return Err("cannot initialize Antalya worker signal mask");
            }
            let mut blocked = blocked.assume_init();
            // Keep synchronous faults and process continuation deliverable,
            // as PostgreSQL's own general blocking mask does.
            for signal in [
                libc::SIGILL,
                libc::SIGFPE,
                libc::SIGSEGV,
                libc::SIGBUS,
                libc::SIGTRAP,
                libc::SIGABRT,
                libc::SIGSYS,
                libc::SIGCONT,
            ] {
                if libc::sigdelset(&mut blocked, signal) != 0 {
                    return Err("cannot initialize Antalya worker signal mask");
                }
            }
            let mut previous = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            if libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, previous.as_mut_ptr()) != 0 {
                return Err("cannot block Antalya worker signals");
            }
            Ok(Self(previous.assume_init()))
        }
    }
}

impl Drop for SignalMask {
    fn drop(&mut self) {
        // SAFETY: this is the initialized mask saved on this thread. The guard
        // stays on the constructing stack, including during Rust unwinding.
        let result =
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut()) };
        // A valid saved mask and SIG_SETMASK cannot fail on supported POSIX
        // systems. Continuing with backend signals disabled would be unsafe.
        if result != 0 {
            std::process::abort();
        }
    }
}

pub(super) fn build_runtime() -> Result<tokio::runtime::Runtime, &'static str> {
    let _mask = SignalMask::block()?;
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        // System DNS uses spawn_blocking; abort cannot stop a running lookup.
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .map_err(|_| "cannot create Antalya runtime")
}

#[cfg(test)]
mod tests {
    use super::super::Session;
    use std::collections::HashMap;

    fn signal_mask(signals: &[libc::c_int]) -> Vec<bool> {
        // SAFETY: pthread_sigmask initializes the provided mask; sigismember
        // reads only that initialized mask and valid POSIX signal numbers.
        unsafe {
            let mut mask = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), mask.as_mut_ptr()),
                0
            );
            let mask = mask.assume_init();
            signals
                .iter()
                .map(|sig| libc::sigismember(&mask, *sig) == 1)
                .collect()
        }
    }

    fn backend_signal_mask() -> Vec<bool> {
        signal_mask(&[
            libc::SIGHUP,
            libc::SIGINT,
            libc::SIGTERM,
            libc::SIGQUIT,
            libc::SIGALRM,
            libc::SIGPIPE,
            libc::SIGUSR1,
            libc::SIGUSR2,
            libc::SIGCHLD,
        ])
    }

    fn fault_signal_mask() -> Vec<bool> {
        signal_mask(&[
            libc::SIGILL,
            libc::SIGFPE,
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGTRAP,
            libc::SIGABRT,
            libc::SIGSYS,
            libc::SIGCONT,
        ])
    }

    #[test]
    fn workers_block_backend_signals_and_creator_mask_is_restored() {
        let before = backend_signal_mask();
        let faults_before = fault_signal_mask();
        let session = Session::from_options(&HashMap::from([(
            "bearer_token".into(),
            "signal-test".into(),
        )]))
        .unwrap();
        assert_eq!(
            backend_signal_mask(),
            before,
            "runtime construction changed backend signal mask"
        );
        let worker = session.runtime.spawn(async {
            let asynchronous = backend_signal_mask();
            // DNS workers are created lazily from an asynchronous worker.
            let blocking = tokio::task::spawn_blocking(backend_signal_mask)
                .await
                .unwrap();
            (asynchronous, blocking, fault_signal_mask())
        });
        let (asynchronous, blocking, faults) = session.runtime.block_on(worker).unwrap();
        assert_eq!(
            faults, faults_before,
            "worker changed synchronous fault signal mask"
        );
        assert!(
            asynchronous.iter().all(|blocked| *blocked),
            "async worker accepts backend signals: {asynchronous:?}"
        );
        assert!(
            blocking.iter().all(|blocked| *blocked),
            "blocking worker accepts backend signals: {blocking:?}"
        );
        assert_eq!(
            backend_signal_mask(),
            before,
            "runtime work changed backend signal mask"
        );
    }

    #[test]
    fn creator_mask_is_restored_during_unwinding() {
        let before = backend_signal_mask();
        let result = std::panic::catch_unwind(|| {
            let _mask = super::SignalMask::block().unwrap();
            assert!(backend_signal_mask().iter().all(|blocked| *blocked));
            panic!("simulate runtime construction failure");
        });
        assert!(result.is_err());
        assert_eq!(backend_signal_mask(), before);
    }
}
