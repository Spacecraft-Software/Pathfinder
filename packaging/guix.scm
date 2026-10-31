;; SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
;; SPDX-License-Identifier: GPL-3.0-or-later

(define-module (pathfinder)
  #:use-module (guix packages)
  #:use-module (guix git-download)
  #:use-module (guix build-system cargo)
  #:use-module ((guix licenses) #:prefix license:))

(define-public pathfinder
  (package
    (name "pathfinder")
    (version "0.1.0")
    (source (local-file "../." "pathfinder-checkout"
                        #:recursive? #t))
    (build-system cargo-build-system)
    (arguments
     (list
      #:install-source? #f
      #:phases
      #~(modify-phases %standard-phases
          ;; The `jq` name is installed as a separate symlink rather than being
          ;; the package's only entry point, so adding this package to a profile
          ;; never silently displaces an existing jq. Drop this phase to get the
          ;; binary alone.
          (add-after 'install 'install-jq-shim
            (lambda* (#:key outputs #:allow-other-keys)
              (let ((bin (string-append (assoc-ref outputs "out") "/bin")))
                (symlink "pathfinder" (string-append bin "/jq")))))
          ;; `mjq`: a short name for Pathfinder itself, which shadows nothing.
          (add-after 'install 'install-mjq-alias
            (lambda* (#:key outputs #:allow-other-keys)
              (let ((bin (string-append (assoc-ref outputs "out") "/bin")))
                (symlink "pathfinder" (string-append bin "/mjq"))))))))
    (home-page "https://Pathfinder.SpacecraftSoftware.org/")
    (synopsis "jq 1.8.1-compatible jq, with its own engine")
    (description
     "Pathfinder accepts jq's command line and runs jq programs as jq 1.8.1
does, on an engine of its own grown from jaq's: jq's numbers, errors,
regular expressions (an Oniguruma port) and time functions.  Installed as
@command{jq} it lets existing scripts keep working.  The few places it
differs from jq are documented rather than silently papered over.")
    (license license:gpl3+)))

pathfinder
