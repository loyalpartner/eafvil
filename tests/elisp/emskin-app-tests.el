;;; emskin-app-tests.el --- Tests for emskin app lifecycle  -*- lexical-binding: t; -*-

(require 'ert)
(require 'emskin)

(ert-deftest emskin-on-window-destroyed-skips-nondeletable-main-window ()
  (let ((emskin--mirror-table (make-hash-table :test 'eql))
        (sent nil))
    (cl-letf (((symbol-function 'emskin--send)
               (lambda (msg) (setq sent msg))))
      (switch-to-buffer (get-buffer-create "*emskin-test*"))
      (display-buffer-in-side-window
       (get-buffer-create "*emskin-side*")
       '((side . right)))
      (with-current-buffer "*emskin-test*"
        (setq-local emskin--window-id 42))
      (should-not
       (condition-case err
           (progn
             (emskin--on-window-destroyed 42)
             nil)
         (error err)))
      (should-not (get-buffer "*emskin-test*"))
      (should (equal (alist-get 'type sent) "set_focus")))))

(provide 'emskin-app-tests)
;;; emskin-app-tests.el ends here
