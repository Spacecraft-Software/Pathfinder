;; SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
;; SPDX-License-Identifier: GPL-3.0-or-later

(define-module (pathfinder)
  #:use-module (guix packages)
  #:use-module (guix git-download)
  #:use-module (guix build-system cargo)
  #:use-module ((guix licenses) #:prefix license:)
  #:use-module (gnu packages jq))

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
                (symlink "pathfinder" (string-append bin "/jq"))))))))
    ;; jaq is a runtime dependency: Pathfinder implements none of the jq
    ;; language and is useless without it.
    (inputs (list jaq))
    (home-page "https://Pathfinder.SpacecraftSoftware.org/")
    (synopsis "jq-compatible shim over jaq")
    (description
     "Pathfinder accepts jq's command line, translates it into jaq's, rewrites
the filter when a missing builtin needs supplying, and hands off to jaq.
Installed as @command{jq} it lets existing scripts keep working while jaq does
the actual work.  Divergences that cannot be repaired from the filter level are
documented rather than silently papered over.")
    (license license:gpl3+)))

pathfinder
