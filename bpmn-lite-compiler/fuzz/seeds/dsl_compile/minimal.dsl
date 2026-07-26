(workflow minimal
  (start-event :id start :next only)
  (service-task :id only :verb cbu.create :next end)
  (end-event :id end :status "Done"))
